# Gust — Agent / Worktree Handoff

> Read this first if you are continuing Gust from another worktree or session.
> Last updated: 2026-08-17

## What Gust is

**Gust** is a Rust load-testing studio. Tagline: *Find where your system falls apart.*

Product bet: win the occupied load-testing category (k6, Vegeta, oha, Goose, loadr) on **craft and clarity**, not novelty. Differentiator = beautiful, correct visualization of system degradation under load — especially the **knee** where p99 detaches from p50.

Not a finance simulator. Not Kafka-first. Kafka/distributed generators only appear in P4 if a real need exists.

## Goals (user priorities)

1. **Real users** — developers actually run it against their APIs
2. **Portfolio signal** — “I built a correct open-model load tester with live degradation UI”
3. **Learning** — async Rust, HDR histograms, coordinated omission, Tokio scheduling, TUI craft

Cost target: **$0** local-first (no paid APIs, no managed infra).

## Current state

| Phase | Status | What |
| --- | --- | --- |
| P0 | **Done** | Open-model constant-rate HTTP load; HDR raw vs coordinated-omission-corrected percentiles |
| P1 | **Done** | Live ratatui dashboard: throughput, percentile table, windowed p50/p90/p99 chart |
| P2 | **Done** | Ramp profiles + automatic breaking-point (knee) detection + HTML report |
| P3 | **Done** | Multi-endpoint / scenarios; in-flight backpressure viz |
| P4 | Maybe | Distributed generators + correct histogram merge |

## Repo layout

```
gust/
├── AGENTS.md              ← you are here
├── README.md              ← product overview + quick start
├── docs/
│   ├── PLAN.md            ← phased plan with acceptance criteria
│   ├── ARCHITECTURE.md    ← crates, data flow, invariants
│   ├── DECISIONS.md       ← why we chose what we chose
│   ├── STATUS.md          ← done / known gaps / next tasks
│   └── HANDOFF.md         ← commands, verify, continue checklist
├── crates/
│   ├── gust-core/         ← pure measurement (no I/O, no async)
│   └── gust-cli/          ← binary: scheduler + reqwest + TUI
└── Cargo.toml             ← workspace (edition 2024)
```

## Non-negotiable invariants

1. **Open-model scheduling** — fire on wall-clock schedule; do not wait for responses to send the next request.
2. **Always report raw AND corrected** percentiles. Coordinated omission is the day-one correctness story.
3. **`gust-core` stays pure** — no Tokio, no HTTP, no TUI. Measurement logic must be unit-testable offline.
4. **Never use floats for money** — N/A here, but never use misleading averages as the headline metric; use HDR percentiles.
5. **Do not bolt on Kafka/Redis** until P4 has a concrete distributed need.
6. **Differentiate from loadr** — loadr.io already ships “Rust load tester + desktop UI”. Gust wins on *finding and explaining the breaking point*, not on being another general-purpose runner.

## Commands that must keep working

```bash
cd /path/to/gust
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --release -p gust

# Demo target with a known capacity (~800 req/s) — the dogfood loop:
node examples/demo-api.js
cargo run --release -p gust -- run http://127.0.0.1:8080/ --profile ramp --from 200 --to 1600 --duration 30 --report ./out.html
cargo run --release -p gust -- run --scenario examples/demo-mix.toml --rate 200 --duration 10 --no-ui
cargo run --release -p gust -- run http://127.0.0.1:8080/ --rate 200 --duration 10   # TUI
```

## Where to continue

Single-node Gust (P0–P3) is complete. Only start **P4** if a real multi-machine need appears — see [`docs/PLAN.md`](docs/PLAN.md). Polish / dogfood tasks are in [`docs/STATUS.md`](docs/STATUS.md).
