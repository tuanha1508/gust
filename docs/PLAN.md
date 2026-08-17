# Gust — Plan

## North star

A local-first Rust CLI + live UI that answers:

> **At what load did this system degrade, and what evidence shows it?**

Not: yet another request blaster with a summary table.
Not: a Stripe clone / bank sim / finance lab.

## Positioning vs competitors

| Tool | Strength | Gust angle |
| --- | --- | --- |
| k6 | Scripting, Grafana ecosystem | Better live craft; clearer CO story |
| Vegeta / oha | Fast CLI | Live degradation UI |
| Goose / rlt | Rust frameworks | Product, not library |
| **loadr** | Closest: Rust + GUI + percentiles | Narrower: **knee detection + explain the break** |

Win with: open-model correctness + beautiful live charts + automatic breaking-point report.

---

## P0 — Correct constant-rate load *(done)*

**Goal:** Hit one URL at fixed RPS; measure honestly.

### Acceptance criteria

- [x] Open-model ticker (`MissedTickBehavior::Burst`)
- [x] HDR histogram 1µs–60s, 3 sig figs
- [x] Raw + coordinated-omission-corrected percentiles
- [x] Unit test: stall inflates corrected p99 vs naive
- [x] CLI: `gust run <url> --rate --duration --timeout`
- [x] `--no-ui` plain summary

### Deliverable

Working binary + `gust-core` tests green.

---

## P1 — Live degradation dashboard *(done)*

**Goal:** Watch the system struggle in real time.

### Acceptance criteria

- [x] ratatui dashboard default
- [x] Throughput / sent / ok% / fail%
- [x] Cumulative raw vs corrected table (highlight CO gaps)
- [x] Windowed p50/p90/p99 time series chart
- [x] Quit: `q` / Esc / Ctrl-C; keep final frame until dismiss

### Deliverable

`gust-cli/src/ui.rs` + windowed `Recorder::take_window()`.

---

## P2 — Ramp + knee + HTML report *(done)*

**Goal:** Automatically find the breaking point and produce a shareable artifact.

### Scope

1. **Load profiles**
   - `constant` (existing)
   - `ramp` — start_rps → end_rps over duration
   - optional: `steps` — hold N seconds at each rate *(deferred)*

2. **Knee / breaking-point detection**
   - While ramping, record per-window: rate, throughput, p50, p99, error rate
   - Detect “knee”: last safe step before latency quality degrades
   - Surface in TUI: marker on chart + “knee ≈ X req/s” banner
   - Print in summary: recommended safe operating load (knee × 0.75)

3. **Shareable HTML report**
   - Single self-contained HTML file (inline CSS/JS, no CDN)
   - Charts: latency series, throughput, knee annotation
   - Table: raw vs corrected percentiles
   - CLI: `gust run ... --report out.html`

### Suggested CLI shape

```bash
gust run http://localhost:8080/ \
  --profile ramp --from 50 --to 2000 --duration 60 \
  --report ./out/gust-report.html
```

### Acceptance criteria

- [x] Ramp profile changes send interval over time correctly (open model still holds)
- [x] Per-window metrics retained for knee algorithm
- [x] Knee estimated and shown in TUI + CLI summary
- [x] HTML report opens offline and is readable
- [x] Unit tests for knee detection on synthetic series
- [x] `--no-ui` still works for CI

### Out of scope for P2

- Multi-URL scenarios
- gRPC / WebSocket
- Distributed workers
- Auth / cookie jars (can stub later)
- `steps` profile (optional; skip unless needed)
- Separate `gust report <run.json>` replay CLI

### Blog post hook

“How to detect the knee of a latency curve without lying about percentiles.”

---

## P3 — Scenarios + backpressure viz *(done)*

**Goal:** Real API journeys, not only `GET /`.

### Scope

- TOML scenario: sequence of requests, weights, think-time
- Shared in-flight stats in UI (backpressure / Little’s Law intuition)
- Single-URL `--method` / `--header` / `--body` bridge

### Acceptance criteria

- [x] Scenario file runs multi-step HTTP (sequence + weighted)
- [x] Live in-flight metrics in TUI (+ HTML series)
- [x] Still open-model where arrival rate is specified

### Blog post hook

“Seeing backpressure: in-flight depth vs latency.”

---

## P4 — Distributed (only if needed) *(NEXT gate)*

**Goal:** Scale past one machine without corrupting percentiles.

### Scope (draft)

- Coordinator + workers
- Merge HDR histograms correctly (not averages of p99s)
- Optional: Redpanda/Kafka for control plane events — **only if** it earns its keep vs plain gRPC/WebSocket

Do **not** start P4 until single-node Gust is useful to real users.

---

## Non-goals (near term)

- Competing with k6 cloud / BlazeMeter scale
- Browser / Web Vitals load testing
- Replacing APM (Datadog, etc.)
- Agent/LLM-specific load tools (kneepoint exists for that)

---

## Success metrics

| Signal | How we know |
| --- | --- |
| Correctness | CO unit test + manual stall demo shows corrected ≫ raw |
| Usefulness | You dogfood it against Visa/backend or local services |
| Craft | Someone screenshots the TUI / HTML report without prompting |
| Portfolio | One clear Show HN / blog: “open model + knee detection” |
