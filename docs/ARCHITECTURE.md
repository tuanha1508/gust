# Gust — Architecture

## Crates

```
┌─────────────────────────────────────────────────────────────┐
│  gust (CLI binary)                                          │
│  crates/gust-cli                                            │
│                                                             │
│  ┌──────────────┐   samples    ┌──────────────────────────┐ │
│  │ Load gen     │ ───────────► │ Recorder task            │ │
│  │ (tokio open  │              │ owns gust_core::Recorder │ │
│  │  model tick) │              │ publishes UiState ~10Hz  │ │
│  └──────┬───────┘              └────────────┬─────────────┘ │
│         │ HTTP                              │ Mutex snapshot│
│         ▼                                   ▼               │
│      reqwest                         ui::dashboard          │
│                                      (ratatui)              │
└─────────────────────────────────────────────────────────────┘
                              │
                              │ types only
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  gust-core (library)                                        │
│  Recorder · Outcome · Percentiles · Summary                 │
│  NO tokio · NO http · NO tui                                │
└─────────────────────────────────────────────────────────────┘
```

## Data flow (current P0/P1)

1. CLI parses `gust run <url> --rate --duration --timeout [--no-ui]`
2. Generator task: `interval(1/rate)` with `MissedTickBehavior::Burst`
3. Each tick spawns a request task (GET via reqwest)
4. Each completion sends `Sample { latency, outcome }` on unbounded mpsc
5. Recorder task:
   - `Recorder::record`
   - every ~100ms: `take_window()` → push `Frame { t, p50, p90, p99 }` into `UiState.series`
   - update cumulative `Summary` + instantaneous throughput
6. UI copies `UiState` under short lock and draws; OR `--no-ui` waits for `finished` and prints summary

## Core types (`gust-core`)

| Type | Role |
| --- | --- |
| `Outcome` | Success / Failure |
| `Recorder` | HDR raw + corrected + window |
| `Percentiles` | min, p50, p90, p99, p999, max (ms) |
| `Summary` | totals + raw + corrected percentiles |
| `LoadProfile` | Constant / Ramp → `rate_at(elapsed)` |
| `WindowMetric` | per-window rate, thr, p50/p90/p99, error_rate, in_flight |
| `Knee` | detected break + recommended safe RPS |
| `SloCapacity` | max sustainable RPS under a p99 budget |
| `RunMetrics` / `CompareReport` | compare inputs + per-metric verdict |
| `Scenario` / `Step` | TOML journey or weighted mix (pure data) |

### Coordinated omission

When `expected_interval` is `Some(d)`:

```text
record(latency)
→ raw.record(micros)
→ window.record(micros)
→ corrected.record_correct(micros, interval_us)   // HDR CO correction
```

When `None`: corrected == raw (closed-model / no planned rate).

### Window

`take_window()` returns percentiles since last call and resets the window histogram. Used for the live chart so bands separate over time instead of showing a dead cumulative average.

## CLI modules

| File | Role |
| --- | --- |
| `main.rs` | clap, profile, generator, recorder_loop, summary, compare, thresholds |
| `ui.rs` | ratatui: header, knee banner, stats, percentile table, latency chart |
| `report.rs` | HTML emitter + JSON artifact load/save |

## Shared UI state

```rust
UiState {
  summary: Option<Summary>,
  series: Vec<WindowMetric>,  // windowed points (latency + target/thr/err)
  throughput: f64,
  target_rps: f64,
  knee: Option<Knee>,
  finished: bool,
}
```

`sent` is a separate `AtomicU64` (generator increments; UI reads).

## Invariants to preserve when extending

1. Generator never waits on response before next tick.
2. Recorder is the only writer of histograms.
3. UI never holds `Mutex` during `terminal.draw` work beyond a quick clone.
4. New metrics for knee detection should land in `gust-core` as pure functions over time series, not buried in `ui.rs`.

## P2 architecture (shipped)

```
LoadProfile::Constant { rate }
LoadProfile::Ramp { from, to, duration }
        │
        ▼
Scheduler accumulates 1/rate_at(t); each Sample carries send interval
        │
        ▼
Recorder + WindowMetric { rate, thr, p50, p90, p99, err_rate }[]
        │
        ▼
knee::detect(&series) -> Option<Knee { rps, reason }>
        │
        ├── TUI marker + banner
        └── report::write_html(...)
```

Modules:

- `gust-core/src/profile.rs` — rate-at-time helpers
- `gust-core/src/knee.rs` — detection algorithm + tests
- `gust-core/src/scenario.rs` — scenario / step types + weighted pick
- `gust-core/src/steps.rs` — `MultiRecorder` overall + per-step histograms
- `gust-core/src/compare.rs` — run metrics, compare verdict, CI thresholds
- `gust-core/src/slo.rs` — SLO-driven capacity over the window series
- `gust-cli/src/report.rs` — HTML + JSON artifacts

### Compare / CI notes

- `--json` writes the same `RunReport` shape embedded in HTML (`schema_version`).
- `gust compare` loads two artifacts, builds `RunMetrics`, calls `compare()`.
- Threshold flags on `gust run` call `check_thresholds()` after the summary.

### SLO capacity notes

- `--slo-p99-ms` computes `slo_capacity(&windows, budget)`: the highest offered
  `target_rps` whose window held p99 ≤ budget before a sustained breach.
- Pure function over `WindowMetric[]`, mirroring `knee::detect`; result is stored
  in the artifact and compared as a metric (higher is better, same budget only).

### P3 notes

- Open-model tick = one **arrival** (journey start or one weighted step).
- Sequence mode records **each HTTP step** as its own sample; `think_ms` sleeps between steps.
- `in_flight` Atomic incremented around each HTTP call; sampled into `WindowMetric` for TUI/HTML.
- Scenario samples carry a step name into `MultiRecorder`; single-URL runs stay overall-only.

Keep HTTP and TUI out of core.

## Performance notes

- Release profile: `opt-level=3`, `lto=thin`, `codegen-units=1`, `panic=abort`
- Prefer measuring the *target*; avoid allocating on the hot path more than necessary
- Unbounded mpsc is fine for P0/P1; if backpressure from recorder becomes an issue under extreme RPS, switch to bounded + drop/count strategy deliberately (document it)

## Toolchain

- Rust / Cargo 1.92+ observed working
- Edition: **2024** (workspace)
- Key crates: tokio, reqwest (rustls), hdrhistogram, clap, ratatui, crossterm
