<p align="center">
  <img src="docs/images/logo.png" alt="" width="128">
</p>

<h1 align="center">
  <img src="docs/images/wordmark.png" alt="Gust" width="396">
</h1>

<p align="center"><strong>Find where your system falls apart.</strong></p>

<p align="center">
  <a href="https://github.com/tuanha1508/gust/actions/workflows/ci.yml"><img src="https://github.com/tuanha1508/gust/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/tuanha1508/gust/releases"><img src="https://img.shields.io/github/v/release/tuanha1508/gust" alt="Release"></a>
  <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License"></a>
</p>

<p align="center">
  Open-model HTTP load tester in Rust.
  Ramp an API, watch p99 detach from p50, and leave with a number you can provision against.
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#why-gust">Why Gust</a> ·
  <a href="#more-commands">Commands</a> ·
  <a href="#docs">Docs</a>
</p>

![Gust HTML report, knee at 806 req/s](docs/images/report-hero.png)

Ramp 200→1600 req/s for 30s against the demo API. **Broke at 806 req/s** (13.0s). **Stay under 604 req/s**.

![Latency over time, p99 leaving the floor at the knee](docs/images/report-latency.png)

p99 sits on the ~11ms service floor, then leaves it at the dashed line and does not come back.

![Throughput flattening while the target keeps climbing](docs/images/report-throughput.png)

The offered rate keeps rising. Completions do not. That is the same break, from the other side.

## Highlights

| | |
| --- | --- |
| **Open-model scheduling** | Fires on a wall-clock schedule. Never waits for a response before sending the next request. |
| **Raw + corrected percentiles** | Coordinated-omission correction sits next to the naive numbers. The gap is the latency users feel. |
| **Knee detection** | Finds where p99 detaches from p50 on a ramp, then names a safe load (75% of knee). |
| **SLO capacity** | `--slo-p99-ms 50` → the max req/s that still holds your p99 budget. |
| **Live TUI + HTML report** | Watch throughput, in-flight depth, and the tail in the terminal; keep a self-contained report. |
| **Compare / CI** | `gust compare` plus exit gates, so a capacity fix is a table, not a screenshot. |

Versus k6, Vegeta, `oha`, Goose: they are fast. Gust wins on showing the breaking point, then proving the fix.

## Install

No Rust toolchain required:

```bash
curl -fsSL https://raw.githubusercontent.com/tuanha1508/gust/main/scripts/install.sh | bash
```

Installs into `~/.local/bin` (override with `DEST=/usr/local/bin`). Archives also live on
[GitHub Releases](https://github.com/tuanha1508/gust/releases) (`macOS arm64/x86_64`, `Linux x86_64/aarch64`).

From source:

```bash
cargo install --git https://github.com/tuanha1508/gust.git --locked gust
# or
git clone https://github.com/tuanha1508/gust.git && cd gust && cargo build --release -p gust
```

## Quick start

The demo API saturates for a real reason: a fixed-size pool. Once arrivals outpace capacity, requests queue. Nothing is faked.

```bash
# Terminal 1 — a target that breaks near ~720 req/s
curl -fsSL https://raw.githubusercontent.com/tuanha1508/gust/main/examples/demo-api.js | node

# Terminal 2 — ramp straight through the breaking point
gust run http://127.0.0.1:8080/ \
  --profile ramp --from 200 --to 1600 --duration 30 \
  --slo-p99-ms 50 --report ./gust-report.html --json ./gust-run.json
```

Open `gust-report.html`. You should see a knee near ~800 req/s, an SLO line, and a written diagnosis.

If you built from source and have not installed the binary, prefix commands with `cargo run --release -p gust --`.

## Why Gust

When a target stalls, most testers stop sending. They never record the wait those blocked requests would have seen, so latency looks best exactly when the system is worst. Gil Tene called this **coordinated omission**.

Gust fires on a wall-clock schedule whether earlier requests have come back or not, then prints **raw vs corrected** on every run. Same demo mix, real numbers:

| | raw | corrected |
| --- | ---: | ---: |
| p50 | 30.46 ms | **184.45 ms** |
| p99 | 742.91 ms | 689.66 ms |

A closed-loop tester would ship the 30ms median. The open-model correction is what users queued through. Full walkthrough: [`docs/KNEE.md`](docs/KNEE.md).

## What you get

### A capacity number, not just a chart

```bash
gust run http://127.0.0.1:8080/ \
  --profile ramp --from 100 --to 1200 --duration 20 --no-ui \
  --slo-p99-ms 50
```

```
  SLO:       p99 ≤ 50ms sustains ≈ 840 req/s (797 served) at t=13.5s
```

It lands in the summary, the HTML banner, and the JSON artifact — so `gust compare` can say the fix in one line:

```
  SLO capacity (req/s)      427.068 →    839.965  (improved, Δ +412.897)
```

### A written diagnosis

Every finished run names the failure mode (*latency saturation*, *throughput collapse*, *error spike*, *dead target*, or *healthy*) and what to do next:

```
  diagnosis: Throughput collapsed near 304 req/s. The system could not keep up with arrivals.
    · knee at 304 req/s (1.5s). p99 73.7ms (4× service floor) and throughput 34% of target
    · stay under 228 req/s (75% of knee)
```

Re-run it from a saved artifact: `gust diagnose ./run.json`.

### Prove a fix in CI

```bash
gust run http://127.0.0.1:8080/ --profile ramp --from 200 --to 1600 --duration 30 --no-ui \
  --json ./run.json --report ./run.html

gust run http://127.0.0.1:8080/ --rate 600 --duration 8 --no-ui \
  --max-p99-ms 50 --max-error-rate 0.01 --min-success-rate 0.99

gust compare ./baseline.json ./after.json            # exit 1 on regress
gust compare ./baseline.json ./after.json --format md
gust report ./run.json -o ./run.html
```

`--format md` is a table a CI job can drop on the pull request:

```
### gust compare

**Verdict: ✅ IMPROVED**

| metric | baseline | candidate | Δ | |
| --- | ---: | ---: | ---: | :--- |
| corrected p99 (ms) | 4943.871 | 1887.231 | -3056.640 | improved |
| knee (req/s) | 411.946 | 808.506 | +396.560 | improved |
| SLO capacity (req/s) | 427.068 | 839.965 | +412.897 | improved |
```

Drop [`examples/gust-perf-gate.yml`](examples/gust-perf-gate.yml) into `.github/workflows/` to run this on every PR. Walkthrough with real before/after numbers (pool 4 → pool 8): [`docs/CASE-STUDY.md`](docs/CASE-STUDY.md).

The live dashboard shows throughput, in-flight depth, the raw-vs-corrected table, latency over time, and a knee banner when the break is detected. Press `q` to stop.

## More commands

```bash
# Live dashboard at a constant rate
gust run http://127.0.0.1:8080/ --rate 200 --duration 10

# Plain summary for scripts / CI
gust run http://127.0.0.1:8080/ --rate 200 --duration 10 --no-ui

# Weighted mix of endpoints
gust run --scenario examples/demo-mix.toml --profile ramp --from 50 --to 500 --duration 25

# Multi-step journey with think-time
gust run --scenario examples/journey.toml --rate 50 --duration 20

# Method / headers / body
gust run http://127.0.0.1:8080/api/items \
  --method POST --header "Content-Type: application/json" --body '{"ok":true}'

# Bearer / Basic
gust run http://127.0.0.1:8080/api/me --bearer demotoken --rate 50 --duration 5 --no-ui
gust run http://127.0.0.1:8080/api/me --basic-auth demo:demo --rate 50 --duration 5 --no-ui

# Login → protected API via cookie jar
gust run --scenario examples/auth-journey.toml --cookie-jar --rate 20 --duration 5 --no-ui
```

Scenario files live in [`examples/`](examples/). Clone the repo (or copy the TOML) for those.

<details>
<summary>Does the knee match ground truth?</summary>

The demo API's capacity is measurable. A constant-rate sweep (`--rate R --duration 8 --no-ui`) against pool 8:

| Rate | raw p99 | Verdict |
| --- | --- | --- |
| 700 req/s | ~11 ms | healthy |
| 750 req/s | ~25 ms | queueing starts |
| 800 req/s | ~180 ms | over capacity |
| 850 req/s | ~670 ms | saturated |

Real breaking point ≈ **720 req/s** (`pool ÷ ~11ms effective service`). A ramp reports a single knee a little higher (~800–870) because it does not dwell long enough for the queue to fully form — treat it as "≈ where it breaks" and operate at the *recommended* load (75% of knee). Full numbers: [`docs/KNEE.md`](docs/KNEE.md).

</details>

## Docs

| | |
| --- | --- |
| [`docs/KNEE.md`](docs/KNEE.md) | Numbers-backed walkthrough of the breaking point |
| [`docs/CASE-STUDY.md`](docs/CASE-STUDY.md) | Baseline → fix → `gust compare` (the portfolio loop) |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Crates, data flow, invariants |
| [`docs/STATUS.md`](docs/STATUS.md) | Done, known gaps, next polish |

`gust-core` is pure measurement — no Tokio, no HTTP, no TUI — so the coordinated-omission logic is unit-tested offline (`cargo test -p gust-core`).

P0–P3 + compare/CI are done. Distributed generators (P4) only if one machine is not enough.

## License

MIT OR Apache-2.0. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
