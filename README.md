# Gust

**Find where your system falls apart.**

Gust is a load-testing studio: point it at an API, ramp up traffic, and watch
the system break in real time — latency distributions spreading, p99 detaching
from p50, throughput flattening while errors climb. It is built in Rust because
a load generator is one of the few tools where *the tool itself* must not be the
bottleneck, and it is designed so the degradation is beautiful to watch, not
just a summary table.

## Why this exists

The category is occupied — k6, Vegeta, `oha`, `rlt`, Goose. None of them win on
being *nice to use*. k6 prints a table. JMeter is ancient Java. The terminal
tools are fast but terminal-only. Gust's bet is the same one TablePlus made
against free SQL clients and `foley` made against other UI-sound libraries:
**win an occupied category on craft, not on novelty.** The differentiator is a
gorgeous, legible view of a system degrading under load.

## The one thing Gust gets right on day one: coordinated omission

Most load testers lie about latency. When a target stalls, a naive tester stops
sending requests and never records the latency those blocked-but-never-sent
requests would have seen — so the distribution looks *best* exactly when the
system is at its worst. This is *coordinated omission* (Gil Tene's term).

Gust uses an **open-model scheduler**: it fires requests on a fixed wall-clock
schedule regardless of whether earlier ones have come back, and it corrects the
histogram against the intended send interval. Every run reports **raw vs
corrected** percentiles side by side. The gap between them is the latency your
users actually feel.

## Try it

Gust ships a demo API that falls apart for a real reason: it serves requests
from a fixed-size pool, so once arrivals outpace capacity, requests queue.
Nothing is faked — the knee you see is genuine queueing.

```bash
# Terminal 1 — a target with capacity ≈ 800 req/s (pool 8 ÷ 10ms service time):
node examples/demo-api.js

# Terminal 2 — ramp straight through the breaking point:
cargo run --release -p gust -- run http://127.0.0.1:8080/ \
  --profile ramp --from 200 --to 1600 --duration 30 \
  --report ./gust-report.html
```

Other things you can point it at:

```bash
# Live dashboard (default) at a constant rate:
cargo run --release -p gust -- run http://127.0.0.1:8080/ --rate 200 --duration 10

# Plain summary for scripts / CI:
cargo run --release -p gust -- run http://127.0.0.1:8080/ --rate 200 --duration 10 --no-ui

# Weighted mix of endpoints with different costs:
cargo run --release -p gust -- run --scenario examples/demo-mix.toml \
  --profile ramp --from 50 --to 500 --duration 25

# Multi-step journey with think-time:
cargo run --release -p gust -- run --scenario examples/journey.toml --rate 50 --duration 20

# Method / headers / body:
cargo run --release -p gust -- run http://127.0.0.1:8080/api/items \
  --method POST --header "Content-Type: application/json" --body '{"ok":true}'
```

The live dashboard shows throughput, in-flight depth, the cumulative raw-vs-corrected
percentile table, latency over time, and a knee banner when the break is detected.
Press `q` to stop.

Real output from the weighted mix above, against the demo API:

```
gust: ramp 50→500 req/s for 25s · demo-mix (3 steps, Weighted)

  arrivals:  6873
  completed: 6873
  success:   6873 (100.0%)
  failure:   0 (0.0%)

  latency (ms)      raw     corrected
  min               9.11          2.02
  p50              30.46        184.45
  p90             446.46        498.94
  p99             742.91        689.66
  p99.9           787.46        756.22
  max             805.89        805.89

  knee:      ≈ 457 req/s at t=22.9s (p99 315.4ms (6× baseline) and throughput 83% of target)
  safe load: ≈ 343 req/s (75% of knee)
```

Read the p50 row twice. A naive tester reports a **30ms median**; the
coordinated-omission correction says users actually waited **184ms**. That gap
is the whole point.

For multi-endpoint scenarios, Gust also breaks latency out **by step** (slowest
corrected p99 first), so you can see which route was holding the pool:

```
  by step (slowest corrected p99 first)
  step                  n      p50      p99   p99 corr
  checkout             63     50.3     51.6       51.4
  search              192     30.9     32.0       31.9
  items               384     10.9     11.8       11.8
```

### Does the knee detection actually work?

Because the demo API's capacity is known arithmetic (`pool ÷ service time`),
the detector can be checked against ground truth:

| Pool | True capacity | Gust's knee | Recommended safe load |
| --- | --- | --- | --- |
| 8 | ~800 req/s | 884 (+10%) | 663 |
| 4 | ~400 req/s | 462 (+15%) | 347 |

The estimate runs slightly high — it reports the last healthy window while the
ramp keeps climbing — so the *recommended* load (75% of knee) stays safely
below real capacity in both cases.

## Architecture

```
gust/
├── AGENTS.md            # handoff for other worktrees / agents (start here)
├── docs/                # PLAN, ARCHITECTURE, DECISIONS, STATUS, HANDOFF
├── examples/            # demo-api.js (capacity-limited target) + scenarios
├── crates/
│   ├── gust-core/       # pure measurement — no I/O, no async, unit-tested
│   └── gust-cli/        # binary — open-model scheduler + reqwest + TUI
└── Cargo.toml
```

The engine is kept free of I/O on purpose: the coordinated-omission logic is the
subtle part, so it is tested without a network (`cargo test -p gust-core`).

**Continuing elsewhere?** Read [`AGENTS.md`](./AGENTS.md) then [`docs/HANDOFF.md`](./docs/HANDOFF.md).

## Roadmap

- **P0 (done):** constant-rate open-model run against one URL; raw vs corrected
  HDR percentiles.
- **P1 (done):** live TUI — windowed p50/p90/p99 chart so the tail visibly
  detaches from the median, throughput readout, and a percentile table that
  flags coordinated-omission gaps.
- **P2 (done):** ramping load profiles; automatic breaking-point (knee)
  detection; self-contained HTML report via `--report`.
- **P3 (done):** TOML scenarios (sequence journeys + weighted mix); in-flight
  backpressure chart; `--method` / `--header` / `--body` on single-URL runs.
- **P4 (only if a real need appears):** distributed generators across machines
  with correctly-merged histograms — the one place a coordination layer earns
  its keep.

## Writing material

Each phase is a post: coordinated omission and why averages lie (P0), rendering
a distribution going bimodal the moment a pool exhausts (P1), detecting the knee
(P2), merging HDR histograms across machines without corrupting percentiles
(P4).

## License

MIT OR Apache-2.0.
