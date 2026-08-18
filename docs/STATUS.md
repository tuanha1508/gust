# Gust — Status

Last updated: **2026-08-19**

## Done

### P0 — Correct constant-rate load

- Workspace: `crates/gust-core`, `crates/gust-cli`
- `Recorder` with HDR raw + CO-corrected + unit tests
- Open-model tokio generator + reqwest GET
- CLI summary with raw vs corrected table
- `--no-ui` flag

### P1 — Live TUI

- `Recorder::take_window()` for chart series
- `ui.rs`: header, stats, percentile table, latency chart
- Quit keys; final frame until dismiss

### P2 — Ramp + knee + HTML report

- `LoadProfile::{Constant, Ramp}` with `rate_at(elapsed)` + tests
- Open-model ramp generator; per-sample CO intervals
- `WindowMetric` + `knee::detect` + TUI/summary/HTML

### P3 — Scenarios + backpressure viz

- TOML scenarios: `mode = "sequence"` (journey + `think_ms`) or `"weighted"`
- Steps: method, url, headers, body, weight
- Single-URL bridge: `--method`, `--header`, `--body`
- Live `in-flight` counter + TUI chart; HTML in-flight series
- **Per-step latency breakdown** (slowest corrected p99 first) in CLI / TUI / HTML
- Examples: `examples/demo-api.js`, `examples/demo-mix.toml`, `examples/journey.toml`
- Open model still schedules arrivals; each arrival runs a journey or one weighted step

### Compare / CI — regression-aware runs

- `--json` writes a schema-versioned run artifact (same payload as the HTML embed)
- `gust compare baseline.json candidate.json` — corrected p99, error rate, knee;
  verdict `IMPROVED` / `EQUIVALENT` / `MIXED` / `REGRESSED` (non-zero on MIXED/REGRESSED)
- `gust compare --format human|md|json` — Markdown table for PR comments, JSON for tooling
- `examples/gust-perf-gate.yml` — drop-in GitHub Action: runs a ramp, compares to a
  committed baseline, posts the Markdown table on the PR, fails the check on regression
- `gust report <run.json> -o out.html` rebuilds HTML without re-running
- CI gates on `gust run`: `--max-p99-ms`, `--max-error-rate`, `--min-success-rate`,
  `--min-knee-rps`, `--require-knee`
- Pure logic in `gust-core::compare` (unit-tested)
- Dogfood case study: [`CASE-STUDY.md`](CASE-STUDY.md) (demo-api pool 4 → 8,
  knee ~407 → ~809 req/s, verdict IMPROVED)

### SLO-driven capacity — the number planners ask for

- `--slo-p99-ms <ms>`: reports the **max offered load that holds p99 under the
  budget** (sustainable req/s + throughput served), before a *sustained* breach
- Pure `gust-core::slo_capacity` over the window series (unit-tested, 6 cases:
  ramp read, tighter/looser budget, single-spike robustness, error disqualify,
  too-short)
- Surfaced in CLI summary, HTML banner, JSON artifact; flows into `gust compare`
  as an `SLO capacity (req/s)` row when both runs used the same budget
- Dogfood: pool 4 → 8 lifted SLO(p99≤50ms) capacity ~427 → ~840 req/s

### Plain-English auto-diagnosis

- Every finished run gets a `Diagnosis`: cause enum + headline + evidence bullets
  + narrative paragraph (latency saturation / throughput collapse / error spike /
  dead target / healthy / insufficient data)
- Pure `gust-core::diagnose` over summary + windows + knee + SLO + first failure
- Surfaced in CLI summary, HTML panel, JSON artifact
- `gust diagnose <run.json>` re-prints from a saved artifact (also backfills
  diagnosis when rebuilding HTML via `gust report`)## Verified working

```bash
cargo test --workspace                          # 16 tests pass
cargo clippy --workspace --all-targets -- -D warnings   # clean
cargo fmt --all -- --check                      # clean
cargo build --release -p gust
```

Dogfooded against `examples/demo-api.js` (bounded pool → real queueing):

- POST with headers + body verified server-side (9/9 bodies correct)
- Sequence scenario: 19 arrivals → 38 HTTP samples
- Weighted scenario exercises all routes
- **Per-step breakdown** against 1×/3×/5× routes: checkout ≈50ms, search ≈31ms, items ≈11ms (correct order)
- HTML report rendered headless (Playwright): 3 charts draw, no JS errors; steps table present

**Capacity measured against ground truth** — constant-rate sweep, pool 8
(`--rate R --duration 8 --no-ui`), reproducible across passes:

| Rate | raw p99 | Verdict |
| --- | --- | --- |
| 700 req/s | ~11 ms | healthy (at service floor) |
| 750 req/s | ~25 ms | queueing starts |
| 800 req/s | ~180 ms | over capacity |
| 850 req/s | ~670 ms | saturated |

Real breaking point ≈ **720 req/s**, matching `pool ÷ effective service`
(8 ÷ 11ms ≈ 727). The naive `8 ÷ 10ms = 800` is a theoretical ceiling; the
extra ~1ms/req is Node's single-threaded event-loop overhead. The ramp knee
lands near this band but is noisier run-to-run than the sweep.

TUI requires a real interactive terminal (not verified in headless agent shell).

## Known gaps / debt

1. ~~**Early quit**~~ — Ctrl-C / `q` now stops scheduling, drains in-flight,
   and prints a full summary + report. Dashboard stays up with a STOPPING
   status until the drain finishes.
2. **Throughput in UI** — instantaneous window estimate; can be noisy at low RPS.
3. **Knee on short/noisy runs** — mitigated: require ≥10 windows, ≥20ms absolute
   p99 rise, and sustained hot windows (or throughput collapse). Unit-tested against
   the prior false-positive fixture.
4. **Ramp knee ≈ steady-state capacity** — a constant-rate sweep is still the
   sharper capacity measurement (queue needs dwell time to form), but the ramp
   knee no longer swings by hundreds of req/s. Mitigated: service-floor baseline
   from the 20th percentile of low-error windows (resists a hot start), last
   healthy window requires p99 near that floor (not merely low errors), and
   gradual climbs fire after 3 sustained hot windows. Dogfooded: 5 back-to-back
   ramps against pool-8 landed 810–871 req/s (stdev ≈23); previously a
   contaminated start reported ~1380.
5. ~~**`corrected` min below `raw` min**~~ — corrected min is now shown as `—`.
6. **Steps profile** — optional ramp variant; not implemented.
7. ~~**`gust report <run.json>`**~~ — `gust report <run.json> -o out.html`
   rebuilds HTML from a saved `--json` artifact.
8. ~~**No cookie jar / auth helpers**~~ — `--bearer`, `--basic-auth`,
   `--cookie`, `--cookie-jar`, plus scenario `[auth]` / `[cookies]` /
   `cookie_jar`. Demo: `POST /login` + `GET /api/me`, `examples/auth-journey.toml`.
9. **crates.io name `gust` is taken** — an unrelated 2017 charting crate
   ([crates.io/crates/gust](https://crates.io/crates/gust)). Do not `cargo
   publish` under that name. Binary stays `gust`; if we publish later, pick a
   distinct package name (e.g. `gust-load`) without renaming the CLI.

## Next tasks

Polish / distribution, in rough value order:
1. ~~**GitHub Releases** binary (macOS/Linux)~~ — `.github/workflows/release.yml`
   on `v*` tags; `scripts/install.sh` one-liner; MIT + Apache licenses
2. Short demo GIF/video of the TUI + compare loop
3. Optional connection-pool wait metrics from reqwest (if exposed)
4. Steps / hold profile if dogfooding wants it
5. Publish under a free crates.io name if/when distribution matters

Distributed generators (old P4) — only if one machine is not enough.
## Intentionally not started

- Kafka / Redis / distributed workers
- Desktop GUI
- gRPC / WS protocols
- Claiming the `gust` crates.io name (already owned)
