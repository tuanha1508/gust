# Case study: prove a capacity fix with `gust compare`

This is the portfolio loop Gust is for: measure a system, change it, and show
**numbers** — not a screenshot of a green CI badge.

The target is the demo API shipped with the repo. It fails for a real reason: a
bounded worker pool. We treat **pool=4** as the broken baseline and **pool=8**
as the fix (doubling concurrency), then let Gust score the before/after.

## Commands

```bash
# Terminal A — constrained capacity
node examples/demo-api.js --port 8080 --pool 4

# Terminal B — baseline artifact
cargo run --release -p gust -- run http://127.0.0.1:8080/ \
  --profile ramp --from 100 --to 1200 --duration 20 --no-ui \
  --json /tmp/gust-baseline.json --report /tmp/gust-baseline.html
```

Restart the demo with a larger pool, then re-run with the same ramp:

```bash
node examples/demo-api.js --port 8080 --pool 8

cargo run --release -p gust -- run http://127.0.0.1:8080/ \
  --profile ramp --from 100 --to 1200 --duration 20 --no-ui \
  --json /tmp/gust-after.json --report /tmp/gust-after.html \
  --min-knee-rps 500 --max-error-rate 0.05

cargo run --release -p gust -- compare /tmp/gust-baseline.json /tmp/gust-after.json
```

## One real result (2026-08-18)

| | pool=4 (baseline) | pool=8 (after) |
| --- | --- | --- |
| Success rate | 50.2% | 100% |
| Corrected p99 | ~4944 ms | ~1966 ms |
| Knee | ≈ **407 req/s** | ≈ **809 req/s** |
| Safe load | ≈ 305 req/s | ≈ 606 req/s |

`gust compare` printed:

```
  corrected p99 (ms)       4943.871 →   1966.079  (improved, Δ -2977.792)
  error rate                  0.498 →      0.000  (improved, Δ -0.498)
  knee (req/s)              406.694 →    808.500  (improved, Δ +401.807)

  verdict: IMPROVED
```

Exit code **0** on improve / equivalent; **1** on regress or mixed — so the
same command is usable as a PR gate after you save a baseline artifact.

## What a recruiter should take away

1. **Correctness** — open-model load + coordinated-omission-corrected percentiles
2. **Diagnosis** — automatic knee naming the load where the system stops being itself
3. **Regression discipline** — JSON artifacts + `gust compare` + CI thresholds
4. **Evidence** — a before/after capacity story with a reproducible demo

Point Gust at an API you own the same way. The demo just makes the ground truth
checkable (`pool ÷ effective service time`).
