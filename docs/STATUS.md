# Gust — Status

Last updated: **2026-08-17**

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

## Verified working

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

**Knee validated against ground truth** (`capacity = pool ÷ service_ms`):

| Pool | True capacity | Detected knee | Safe load |
| --- | --- | --- | --- |
| 8 | ~800 req/s | 884 (+10%) | 663 |
| 4 | ~400 req/s | 462 (+15%) | 347 |

TUI requires a real interactive terminal (not verified in headless agent shell).

## Known gaps / debt

1. **Early quit** — aborting generator may leave in-flight requests; summary may be partial.
2. **Throughput in UI** — instantaneous window estimate; can be noisy at low RPS.
3. **Knee on short/noisy runs** — mitigated: require ≥10 windows, ≥20ms absolute
   p99 rise, and sustained hot windows (or throughput collapse). Unit-tested against
   the prior false-positive fixture.
4. **Knee reads ~10–15% above true capacity** — it reports the last healthy window
   while the ramp keeps climbing. Safe-load advice (75%) still lands under capacity.
5. ~~**`corrected` min below `raw` min**~~ — corrected min is now shown as `—`.
6. **Steps profile** — optional ramp variant; not implemented.
7. **`gust report <run.json>`** — HTML written from live run only.
8. **No cookie jar / auth helpers** — headers only.
9. **No `cargo publish` / crates.io claim** for `gust` yet.

## Next tasks (P4) — only if needed

Distributed generators + correct HDR merge. Do **not** start until single-node Gust has real users.

Polish first, in rough value order:
1. Optional connection-pool wait metrics from reqwest (if exposed)
2. Steps / hold profile if dogfooding wants it

## Intentionally not started

- Kafka / Redis / distributed workers
- Desktop GUI
- gRPC / WS protocols
