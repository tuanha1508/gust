//! Gust — find where your system falls apart.
//!
//! Open-model HTTP load with HDR raw vs coordinated-omission-corrected
//! percentiles, live TUI, ramp profiles, knee detection, scenarios, and
//! HTML reports.

mod report;
mod ui;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use gust_core::{
    Knee, LoadProfile, MultiRecorder, Outcome, Scenario, ScenarioAuth, ScenarioMode, Step,
    StepSummary, Summary, WindowMetric, detect_knee,
};
use reqwest::Method;
use reqwest::cookie::Jar;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::sync::mpsc;

use report::RunReport;

#[derive(Parser)]
#[command(name = "gust", version, about = "Find where your system falls apart.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProfileKind {
    Constant,
    Ramp,
}

#[derive(Subcommand)]
enum Command {
    /// Drive open-model load at a URL or scenario for a fixed duration.
    Run {
        /// Target URL (required unless `--scenario` is set).
        #[arg(required_unless_present = "scenario")]
        url: Option<String>,

        /// TOML scenario file (sequence or weighted steps).
        #[arg(long)]
        scenario: Option<PathBuf>,

        /// HTTP method for single-URL runs (default GET).
        #[arg(long, default_value = "GET")]
        method: String,

        /// Extra header `Name: value` (repeatable) for single-URL runs.
        #[arg(long = "header", value_name = "NAME: VALUE")]
        headers: Vec<String>,

        /// Request body for single-URL runs (e.g. JSON).
        #[arg(long)]
        body: Option<String>,

        /// `Authorization: Bearer <token>` (single-URL; also fills scenario auth if unset).
        #[arg(long = "bearer", value_name = "TOKEN")]
        bearer: Option<String>,

        /// HTTP Basic auth as `user:password` (single-URL; also fills scenario auth if unset).
        #[arg(long = "basic-auth", value_name = "USER:PASSWORD")]
        basic_auth: Option<String>,

        /// Seed cookie `name=value` (repeatable).
        #[arg(long = "cookie", value_name = "NAME=VALUE")]
        cookies: Vec<String>,

        /// Persist `Set-Cookie` across requests (needed for login → API journeys).
        #[arg(long)]
        cookie_jar: bool,

        /// Load profile: constant RPS or linear ramp.
        #[arg(long, value_enum, default_value_t = ProfileKind::Constant)]
        profile: ProfileKind,

        /// Requests (or journeys) per second — constant profile.
        #[arg(short, long, default_value_t = 50)]
        rate: u64,

        /// Ramp start RPS (`--profile ramp`).
        #[arg(long)]
        from: Option<f64>,

        /// Ramp end RPS (`--profile ramp`).
        #[arg(long)]
        to: Option<f64>,

        /// How long to run, in seconds.
        #[arg(short, long, default_value_t = 10)]
        duration: u64,

        /// Per-request timeout, in seconds.
        #[arg(short, long, default_value_t = 5)]
        timeout: u64,

        /// Print a plain summary instead of the live dashboard.
        #[arg(long)]
        no_ui: bool,

        /// Write a self-contained HTML report to this path when the run ends.
        #[arg(long)]
        report: Option<PathBuf>,
    },
}

/// What each open-model arrival executes.
#[derive(Clone)]
enum Target {
    Single(Step),
    Scenario(Scenario),
}

impl Target {
    fn label(&self) -> String {
        match self {
            Self::Single(s) => format!("{} {}", s.method, s.url),
            Self::Scenario(s) => s.label(),
        }
    }
}

/// One completed request's measurement, sent from a worker to the recorder.
struct Sample {
    latency: Duration,
    outcome: Outcome,
    expected_interval: Duration,
    target_rps: f64,
    /// Scenario step name; `None` for anonymous single-URL runs.
    step: Option<String>,
}

/// Snapshot the UI reads each render tick. Written only by the recorder task.
#[derive(Default)]
pub struct UiState {
    pub summary: Option<Summary>,
    pub steps: Vec<StepSummary>,
    pub series: Vec<WindowMetric>,
    pub throughput: f64,
    pub target_rps: f64,
    pub in_flight: u64,
    pub knee: Option<Knee>,
    /// Stop requested: no new arrivals; waiting for in-flight to finish.
    pub stopping: bool,
    pub finished: bool,
}

/// Shared, read-mostly configuration the UI shows in its header.
pub struct RunInfo {
    pub url: String,
    pub profile_label: String,
    pub duration: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run {
            url,
            scenario,
            method,
            headers,
            body,
            bearer,
            basic_auth,
            cookies,
            cookie_jar,
            profile,
            rate,
            from,
            to,
            duration,
            timeout,
            no_ui,
            report,
        } => {
            let cli_auth = CliAuth::from_args(bearer, basic_auth, cookies, cookie_jar)?;
            let (target, shared) = build_target(url, scenario, method, headers, body, &cli_auth)?;
            let load = build_profile(profile, rate, from, to, duration)?;
            run(target, shared, load, timeout, no_ui, report).await
        }
    }
}

fn build_target(
    url: Option<String>,
    scenario_path: Option<PathBuf>,
    method: String,
    headers: Vec<String>,
    body: Option<String>,
    cli_auth: &CliAuth,
) -> Result<(Target, SharedAuth)> {
    if let Some(path) = scenario_path {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("read scenario {}", path.display()))?;
        let raw: Scenario =
            toml::from_str(&text).with_context(|| format!("parse scenario {}", path.display()))?;
        let mut scenario = raw
            .validate()
            .map_err(|e| anyhow::anyhow!("invalid scenario: {e}"))?;
        merge_cli_auth_into_scenario(&mut scenario, cli_auth)?;
        let shared = SharedAuth::from_scenario(&scenario);
        return Ok((Target::Scenario(scenario), shared));
    }

    let url = url.ok_or_else(|| anyhow::anyhow!("url is required without --scenario"))?;
    let header_map = parse_headers(&headers)?;
    let mut step = Step {
        name: "url".into(),
        method,
        url,
        weight: 1,
        think_ms: 0,
        body,
        headers: header_map,
    };
    let mut validated = Scenario {
        name: "single".into(),
        mode: ScenarioMode::Sequence,
        steps: vec![step.clone()],
        auth: ScenarioAuth::default(),
        cookies: BTreeMap::new(),
        cookie_jar: false,
    }
    .validate()
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    merge_cli_auth_into_scenario(&mut validated, cli_auth)?;
    let shared = SharedAuth::from_scenario(&validated);
    step = validated.steps.into_iter().next().unwrap();
    Ok((Target::Single(step), shared))
}

/// CLI auth/cookie flags. Scenario-file values win when already set.
#[derive(Debug, Clone, Default)]
struct CliAuth {
    bearer: Option<String>,
    basic: Option<(String, String)>,
    cookies: BTreeMap<String, String>,
    cookie_jar: bool,
}

impl CliAuth {
    fn from_args(
        bearer: Option<String>,
        basic_auth: Option<String>,
        cookies: Vec<String>,
        cookie_jar: bool,
    ) -> Result<Self> {
        if bearer.is_some() && basic_auth.is_some() {
            bail!("use --bearer or --basic-auth, not both");
        }
        let basic = match basic_auth {
            None => None,
            Some(raw) => {
                let (user, pass) = raw
                    .split_once(':')
                    .ok_or_else(|| anyhow::anyhow!("--basic-auth must be `user:password`"))?;
                if user.is_empty() {
                    bail!("--basic-auth user must not be empty");
                }
                Some((user.to_string(), pass.to_string()))
            }
        };
        if let Some(tok) = &bearer
            && tok.is_empty()
        {
            bail!("--bearer must not be empty");
        }
        let mut jar_cookies = BTreeMap::new();
        for c in cookies {
            let (name, value) = c
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("--cookie must be `name=value`, got `{c}`"))?;
            if name.is_empty() {
                bail!("--cookie name must not be empty");
            }
            if value.contains(';') {
                bail!("--cookie value must not contain `;`");
            }
            jar_cookies.insert(name.to_string(), value.to_string());
        }
        Ok(Self {
            bearer,
            basic,
            cookies: jar_cookies,
            cookie_jar,
        })
    }
}

fn merge_cli_auth_into_scenario(scenario: &mut Scenario, cli: &CliAuth) -> Result<()> {
    if scenario.auth.is_empty() {
        if let Some(tok) = &cli.bearer {
            scenario.auth.bearer = Some(tok.clone());
        } else if let Some((u, p)) = &cli.basic {
            scenario.auth.user = Some(u.clone());
            scenario.auth.password = Some(p.clone());
        }
    } else if cli.bearer.is_some() || cli.basic.is_some() {
        bail!("scenario already sets [auth]; omit --bearer / --basic-auth");
    }
    scenario
        .auth
        .validate()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    for (k, v) in &cli.cookies {
        scenario
            .cookies
            .entry(k.clone())
            .or_insert_with(|| v.clone());
    }
    if cli.cookie_jar {
        scenario.cookie_jar = true;
    }
    Ok(())
}

/// Auth + cookies resolved for a run (from scenario file and/or CLI flags).
#[derive(Debug, Clone, Default)]
struct SharedAuth {
    auth: ScenarioAuth,
    cookies: BTreeMap<String, String>,
    cookie_jar: bool,
}

impl SharedAuth {
    fn from_scenario(s: &Scenario) -> Self {
        Self {
            auth: s.auth.clone(),
            cookies: s.cookies.clone(),
            cookie_jar: s.cookie_jar,
        }
    }

    fn seed_url<'a>(&self, target: &'a Target) -> &'a str {
        match target {
            Target::Single(step) => step.url.as_str(),
            Target::Scenario(s) => s.steps[0].url.as_str(),
        }
    }
}

/// Per-request auth applied by workers (jar cookies live on the `Client`).
#[derive(Clone, Default)]
struct Session {
    bearer: Option<String>,
    basic: Option<(String, String)>,
    /// Static `Cookie` header when the jar is off.
    cookie_header: Option<String>,
}

impl Session {
    fn from_shared(shared: &SharedAuth) -> Self {
        let cookie_header = if shared.cookie_jar || shared.cookies.is_empty() {
            None
        } else {
            Some(
                shared
                    .cookies
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        };
        Self {
            bearer: shared.auth.bearer.clone(),
            basic: match (&shared.auth.user, &shared.auth.password) {
                (Some(u), Some(p)) => Some((u.clone(), p.clone())),
                _ => None,
            },
            cookie_header,
        }
    }
}

fn build_http_client(
    timeout_secs: u64,
    target: &Target,
    shared: &SharedAuth,
) -> Result<(reqwest::Client, Session)> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .pool_max_idle_per_host(10_000);

    if shared.cookie_jar {
        let jar = Arc::new(Jar::default());
        if let Ok(url) = reqwest::Url::parse(shared.seed_url(target)) {
            for (name, value) in &shared.cookies {
                jar.add_cookie_str(&format!("{name}={value}; Path=/"), &url);
            }
        }
        builder = builder.cookie_provider(jar);
    }

    let client = builder.build()?;
    Ok((client, Session::from_shared(shared)))
}

fn parse_headers(raw: &[String]) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for h in raw {
        let (k, v) = h
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("header must be `Name: value`, got `{h}`"))?;
        let k = k.trim();
        let v = v.trim();
        if k.is_empty() {
            bail!("header name is empty in `{h}`");
        }
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}

fn build_profile(
    kind: ProfileKind,
    rate: u64,
    from: Option<f64>,
    to: Option<f64>,
    duration_secs: u64,
) -> Result<LoadProfile> {
    let duration = Duration::from_secs(duration_secs);
    match kind {
        ProfileKind::Constant => {
            if rate == 0 {
                bail!("rate must be greater than 0");
            }
            Ok(LoadProfile::Constant {
                rate: rate as f64,
                duration,
            })
        }
        ProfileKind::Ramp => {
            let from = from.unwrap_or(rate as f64);
            let to = to.unwrap_or((rate as f64) * 10.0);
            if from < 0.0 || to <= 0.0 {
                bail!("ramp --from/--to must be positive (to > 0)");
            }
            if duration_secs == 0 {
                bail!("duration must be greater than 0");
            }
            Ok(LoadProfile::Ramp { from, to, duration })
        }
    }
}

async fn run(
    target: Target,
    shared: SharedAuth,
    profile: LoadProfile,
    timeout_secs: u64,
    no_ui: bool,
    report_path: Option<PathBuf>,
) -> Result<()> {
    let total_duration = profile.duration();
    let duration_secs = total_duration.as_secs().max(1);

    let (client, session) = build_http_client(timeout_secs, &target, &shared)?;

    let (tx, rx) = mpsc::unbounded_channel::<Sample>();
    let sent = Arc::new(AtomicU64::new(0));
    let in_flight = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new(UiState::default()));

    let info = RunInfo {
        url: target.label(),
        profile_label: profile.label(),
        duration: duration_secs,
    };

    let started_at = iso_now();
    // Bound how long we wait for stragglers after stop: one request timeout
    // plus a little slack for the final samples to reach the recorder.
    let drain_limit = Duration::from_secs(timeout_secs.saturating_add(2));

    let recorder_handle = {
        let state = Arc::clone(&state);
        let in_flight = Arc::clone(&in_flight);
        let initial_rate = profile.start_rate().max(0.1);
        let initial_interval = Duration::from_secs_f64(1.0 / initial_rate);
        tokio::spawn(async move { recorder_loop(rx, state, in_flight, initial_interval).await })
    };

    let generator = {
        let sent = Arc::clone(&sent);
        let in_flight = Arc::clone(&in_flight);
        let stop = Arc::clone(&stop);
        let profile = profile.clone();
        tokio::spawn(async move {
            generate_load(LoadGen {
                client,
                session,
                target,
                profile,
                tx,
                sent,
                in_flight,
                stop,
                drain_limit,
            })
            .await;
        })
    };

    if no_ui {
        println!(
            "gust: {} for {}s · {}",
            info.profile_label, info.duration, info.url
        );
        let mut generator = generator;
        tokio::select! {
            res = &mut generator => { let _ = res; }
            _ = tokio::signal::ctrl_c() => {
                println!("  stopping — draining in-flight requests…");
                request_stop(&stop, &state);
                let _ = generator.await;
            }
        }
        wait_finished(&state).await;
    } else {
        ui::run_dashboard(
            &info,
            Arc::clone(&sent),
            Arc::clone(&state),
            Arc::clone(&stop),
        )?;
        // Dashboard returns only after the run has finished (natural end or
        // drain after q). Make sure the generator is not left hanging.
        request_stop(&stop, &state);
        let _ = generator.await;
        wait_finished(&state).await;
    }

    let _ = recorder_handle.await;

    let (final_summary, steps, windows, knee) = {
        let s = state.lock().unwrap();
        (s.summary, s.steps.clone(), s.series.clone(), s.knee.clone())
    };

    if let Some(summary) = final_summary {
        let sent_n = sent.load(Ordering::Relaxed);
        print_summary(sent_n, &summary, &steps, knee.as_ref());

        if let Some(path) = report_path {
            let report = RunReport {
                url: info.url.clone(),
                profile: info.profile_label.clone(),
                duration_secs: info.duration,
                sent: sent_n,
                started_at,
                summary,
                steps,
                windows,
                knee: knee.clone(),
                failure_reason: first_failure().map(str::to_owned),
            };
            report::write_html(&path, &report)?;
            println!("  report:    {}", path.display());
        }
    }
    Ok(())
}

/// Inputs for the open-model generator task.
struct LoadGen {
    client: reqwest::Client,
    session: Session,
    target: Target,
    profile: LoadProfile,
    tx: mpsc::UnboundedSender<Sample>,
    sent: Arc<AtomicU64>,
    in_flight: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    drain_limit: Duration,
}

/// Open-model generator: each tick starts one arrival (single request or journey).
///
/// When `stop` is set (Ctrl-C / `q`), scheduling ends immediately and we wait for
/// in-flight requests to finish recording before dropping the sample channel —
/// so the summary and report cover everything that was actually sent.
async fn generate_load(g: LoadGen) {
    let LoadGen {
        client,
        session,
        target,
        profile,
        tx,
        sent,
        in_flight,
        stop,
        drain_limit,
    } = g;
    let start = Instant::now();
    let total = profile.duration();
    let mut planned_s = 0.0f64;
    let mut arrival: u64 = 0;

    while start.elapsed() < total && !stop.load(Ordering::Relaxed) {
        let elapsed = start.elapsed();
        let rate = profile.rate_at(elapsed).max(0.1);
        let interval = Duration::from_secs_f64(1.0 / rate);
        planned_s += 1.0 / rate;

        let due = start + Duration::from_secs_f64(planned_s);
        let now = Instant::now();
        if due > now {
            tokio::select! {
                _ = tokio::time::sleep(due - now) => {}
                _ = wait_until_stopped(&stop) => break,
            }
        }
        if start.elapsed() >= total || stop.load(Ordering::Relaxed) {
            break;
        }

        let client = client.clone();
        let session = session.clone();
        let target = target.clone();
        let tx = tx.clone();
        let in_flight = Arc::clone(&in_flight);
        let n = arrival;
        arrival += 1;

        tokio::spawn(async move {
            match &target {
                Target::Single(step) => {
                    // Single-URL runs stay overall-only; no per-step table noise.
                    fire_one(
                        &client, &session, step, interval, rate, &tx, &in_flight, false,
                    )
                    .await;
                }
                Target::Scenario(sc) => match sc.mode {
                    ScenarioMode::Weighted => {
                        let step = sc.pick_weighted(n);
                        fire_one(
                            &client, &session, step, interval, rate, &tx, &in_flight, true,
                        )
                        .await;
                    }
                    ScenarioMode::Sequence => {
                        for (i, step) in sc.steps.iter().enumerate() {
                            fire_one(
                                &client, &session, step, interval, rate, &tx, &in_flight, true,
                            )
                            .await;
                            if step.think_ms > 0 && i + 1 < sc.steps.len() {
                                tokio::time::sleep(Duration::from_millis(step.think_ms)).await;
                            }
                        }
                    }
                },
            }
        });
        sent.fetch_add(1, Ordering::Relaxed);
    }

    drain_in_flight(&in_flight, drain_limit).await;
    // `tx` drops here: once every worker has sent its sample and dropped its
    // clone, the recorder sees end-of-stream and finalizes.
}

async fn wait_until_stopped(stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Wait until nothing is in flight (or the deadline hits). Decrement happens
/// after each sample is sent, so zero means the recorder has every arrival.
async fn drain_in_flight(in_flight: &AtomicU64, limit: Duration) {
    let start = Instant::now();
    while in_flight.load(Ordering::Relaxed) > 0 && start.elapsed() < limit {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn request_stop(stop: &AtomicBool, state: &Mutex<UiState>) {
    stop.store(true, Ordering::Relaxed);
    if let Ok(mut s) = state.lock() {
        s.stopping = true;
    }
}

#[allow(clippy::too_many_arguments)]
async fn fire_one(
    client: &reqwest::Client,
    session: &Session,
    step: &Step,
    interval: Duration,
    rate: f64,
    tx: &mpsc::UnboundedSender<Sample>,
    in_flight: &AtomicU64,
    tag_step: bool,
) {
    in_flight.fetch_add(1, Ordering::Relaxed);
    let t0 = Instant::now();
    let outcome = execute_http(client, session, step).await;
    let latency = t0.elapsed();
    let _ = tx.send(Sample {
        latency,
        outcome,
        expected_interval: interval,
        target_rps: rate,
        step: tag_step.then(|| step.name.clone()),
    });
    // Decrement after send so a drain waiting on in_flight==0 cannot return
    // before the sample is in the channel.
    in_flight.fetch_sub(1, Ordering::Relaxed);
}

async fn execute_http(client: &reqwest::Client, session: &Session, step: &Step) -> Outcome {
    let method = match step.method.as_str() {
        "GET" => Method::GET,
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        "PATCH" => Method::PATCH,
        "DELETE" => Method::DELETE,
        "HEAD" => Method::HEAD,
        _ => Method::GET,
    };

    let mut req = client.request(method, &step.url);
    if !step.headers.is_empty() {
        let mut map = HeaderMap::new();
        for (k, v) in &step.headers {
            if let (Ok(name), Ok(val)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                map.insert(name, val);
            }
        }
        req = req.headers(map);
    }
    // Step headers win if they already set Authorization / Cookie.
    let has_auth = step
        .headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("authorization"));
    let has_cookie = step
        .headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("cookie"));

    if !has_auth {
        if let Some(token) = &session.bearer {
            req = req.bearer_auth(token);
        } else if let Some((user, pass)) = &session.basic {
            req = req.basic_auth(user, Some(pass));
        }
    }
    if !has_cookie && let Some(cookie) = &session.cookie_header {
        req = req.header(reqwest::header::COOKIE, cookie);
    }
    if let Some(body) = &step.body {
        req = req.body(body.clone());
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => Outcome::Success,
        Ok(resp) => {
            note_failure(|| format!("HTTP {}", resp.status()));
            Outcome::Failure
        }
        Err(e) => {
            note_failure(|| describe_transport_error(&e));
            Outcome::Failure
        }
    }
}

/// First reason a request failed, kept so a run can explain itself instead of
/// reporting a bare failure count.
static FIRST_FAILURE: OnceLock<String> = OnceLock::new();

/// Record the reason once. The closure only runs for the first failure, so a
/// fully-failing run does not pay for formatting on every request.
fn note_failure(reason: impl FnOnce() -> String) {
    if FIRST_FAILURE.get().is_none() {
        let _ = FIRST_FAILURE.set(reason());
    }
}

pub fn first_failure() -> Option<&'static str> {
    FIRST_FAILURE.get().map(String::as_str)
}

fn describe_transport_error(e: &reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "request timed out"
    } else if e.is_connect() {
        "could not connect"
    } else if e.is_body() || e.is_decode() {
        "could not read response"
    } else {
        "transport error"
    };
    // reqwest wraps hyper wraps std::io; the innermost source carries the
    // detail that actually tells you what to fix ("connection refused").
    let mut src: &dyn std::error::Error = e;
    while let Some(next) = src.source() {
        src = next;
    }
    format!("{kind}: {src}")
}

async fn recorder_loop(
    mut rx: mpsc::UnboundedReceiver<Sample>,
    state: Arc<Mutex<UiState>>,
    in_flight: Arc<AtomicU64>,
    initial_interval: Duration,
) {
    let mut recorder = MultiRecorder::new(Some(initial_interval));
    let mut snap = tokio::time::interval(Duration::from_millis(100));
    snap.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut publish_state = PublishState {
        start: Instant::now(),
        last_completed: 0,
        last_t: 0.0,
    };
    let mut completed = 0u64;
    let mut last_target_rps = 0.0f64;

    loop {
        tokio::select! {
            maybe = rx.recv() => {
                match maybe {
                    Some(s) => {
                        last_target_rps = s.target_rps;
                        recorder.record_with_interval(
                            s.latency,
                            s.outcome,
                            s.expected_interval,
                            s.step.as_deref(),
                        );
                        completed += 1;
                        while let Ok(s) = rx.try_recv() {
                            last_target_rps = s.target_rps;
                            recorder.record_with_interval(
                                s.latency,
                                s.outcome,
                                s.expected_interval,
                                s.step.as_deref(),
                            );
                            completed += 1;
                        }
                    }
                    None => break,
                }
            }
            _ = snap.tick() => {
                publish(
                    &state,
                    &mut recorder,
                    &in_flight,
                    &mut publish_state,
                    completed,
                    last_target_rps,
                    false,
                );
            }
        }
    }

    publish(
        &state,
        &mut recorder,
        &in_flight,
        &mut publish_state,
        completed,
        last_target_rps,
        true,
    );
}

struct PublishState {
    start: Instant,
    last_completed: u64,
    last_t: f64,
}

fn publish(
    state: &Arc<Mutex<UiState>>,
    recorder: &mut MultiRecorder,
    in_flight: &AtomicU64,
    publish_state: &mut PublishState,
    completed: u64,
    target_rps: f64,
    finished: bool,
) {
    let now = publish_state.start.elapsed().as_secs_f64();
    let dt = (now - publish_state.last_t).max(1e-6);
    let throughput = (completed - publish_state.last_completed) as f64 / dt;
    publish_state.last_completed = completed;
    publish_state.last_t = now;

    let flying = in_flight.load(Ordering::Relaxed);
    let window = recorder.take_window();
    let breakdown = recorder.breakdown();

    let mut s = state.lock().unwrap();
    s.summary = Some(breakdown.overall);
    s.steps = breakdown.steps;
    s.throughput = throughput;
    s.target_rps = target_rps;
    s.in_flight = flying;
    if let Some(w) = window {
        let p = w.percentiles;
        s.series.push(WindowMetric {
            t: now,
            target_rps,
            throughput,
            p50_ms: p.p50_ms,
            p90_ms: p.p90_ms,
            p99_ms: p.p99_ms,
            error_rate: w.error_rate(),
            in_flight: flying as f64,
        });
        s.knee = detect_knee(&s.series);
    }
    if finished {
        s.finished = true;
        s.knee = detect_knee(&s.series);
    }
}

async fn wait_finished(state: &Arc<Mutex<UiState>>) {
    loop {
        if state.lock().unwrap().finished {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn print_summary(sent: u64, s: &Summary, steps: &[StepSummary], knee: Option<&Knee>) {
    let pct = |n: u64| {
        if s.total == 0 {
            0.0
        } else {
            n as f64 / s.total as f64 * 100.0
        }
    };

    println!();
    println!("  arrivals:  {sent}");
    println!("  completed: {}", s.total);
    println!("  success:   {} ({:.1}%)", s.success, pct(s.success));
    println!("  failure:   {} ({:.1}%)", s.failure, pct(s.failure));
    println!();
    println!("  latency (ms)      raw     corrected");
    // Corrected min is misleading: HDR CO backfill can invent values below the
    // observed minimum. Show raw only for that row.
    println!("  {:<12} {:>9.2} {:>13}", "min", s.raw.min_ms, "—");
    row("p50", s.raw.p50_ms, s.corrected.p50_ms);
    row("p90", s.raw.p90_ms, s.corrected.p90_ms);
    row("p99", s.raw.p99_ms, s.corrected.p99_ms);
    row("p99.9", s.raw.p999_ms, s.corrected.p999_ms);
    row("max", s.raw.max_ms, s.corrected.max_ms);

    if steps.len() > 1 {
        println!();
        println!("  by step (slowest corrected p99 first)");
        println!(
            "  {:<14} {:>8} {:>8} {:>8} {:>10}",
            "step", "n", "p50", "p99", "p99 corr"
        );
        for st in steps {
            let ss = &st.summary;
            println!(
                "  {:<14} {:>8} {:>8.1} {:>8.1} {:>10.1}",
                truncate(&st.name, 14),
                ss.total,
                ss.raw.p50_ms,
                ss.raw.p99_ms,
                ss.corrected.p99_ms
            );
        }
    }

    println!();
    if let Some(reason) = first_failure() {
        println!("  first failure: {reason}");
    }
    if s.total > 0 && s.success == 0 {
        println!("  no capacity measured: every request failed, so none of the");
        println!("  latency above reflects work your target actually did.");
        println!();
    } else if let Some(k) = knee {
        println!(
            "  knee:      ≈ {:.0} req/s at t={:.1}s ({})",
            k.target_rps, k.t, k.reason
        );
        println!(
            "  safe load: ≈ {:.0} req/s (75% of knee)",
            k.recommended_rps
        );
        println!();
    }
    println!("  'corrected' accounts for coordinated omission — the gap is the");
    println!("  latency your users feel that a naive tester would hide.");
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn row(label: &str, raw: f64, corrected: f64) {
    println!("  {label:<12} {raw:>9.2} {corrected:>13.2}");
}

/// UTC timestamp as `YYYY-MM-DD HH:MM:SS UTC`, without pulling in a date crate.
fn iso_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// Howard Hinnant's `civil_from_days`: days since the Unix epoch → (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(1), (1970, 1, 2));
        // Leap day, and the day after, in a divisible-by-400 leap year.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        // 2100 is not a leap year: Feb 28 → Mar 1.
        assert_eq!(civil_from_days(47_540), (2100, 2, 28));
        assert_eq!(civil_from_days(47_541), (2100, 3, 1));
        // 2026-08-17, the date this was written.
        assert_eq!(civil_from_days(20_682), (2026, 8, 17));
    }

    #[test]
    fn parse_headers_splits_on_first_colon() {
        let h = parse_headers(&["X-A: 1".into(), "X-Url: http://x/y".into()]).unwrap();
        assert_eq!(h["X-A"], "1");
        assert_eq!(h["X-Url"], "http://x/y");
        assert!(parse_headers(&["nocolon".into()]).is_err());
        assert!(parse_headers(&[": empty".into()]).is_err());
    }

    #[test]
    fn cli_auth_parses_basic_bearer_and_cookies() {
        let a = CliAuth::from_args(
            Some("tok".into()),
            None,
            vec!["sid=abc".into(), "theme=dark".into()],
            true,
        )
        .unwrap();
        assert_eq!(a.bearer.as_deref(), Some("tok"));
        assert!(a.basic.is_none());
        assert_eq!(a.cookies["sid"], "abc");
        assert!(a.cookie_jar);

        let b = CliAuth::from_args(None, Some("demo:s3cret".into()), vec![], false).unwrap();
        assert_eq!(
            b.basic.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
            Some(("demo", "s3cret"))
        );

        assert!(CliAuth::from_args(Some("t".into()), Some("u:p".into()), vec![], false).is_err());
        assert!(CliAuth::from_args(None, Some("nopass".into()), vec![], false).is_err());
        assert!(CliAuth::from_args(None, None, vec!["bad".into()], false).is_err());
    }
}
