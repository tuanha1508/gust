# Gust — Handoff checklist

Use this when opening Gust in another Cursor worktree / machine / agent session.

## 1. Open the right folder

```bash
cd ~/Desktop/gust
```

## 2. Read in this order

1. [`../AGENTS.md`](../AGENTS.md) — invariants + goals  
2. [`STATUS.md`](STATUS.md) — what’s done / next  
3. [`PLAN.md`](PLAN.md) — P4 only if needed  
4. [`ARCHITECTURE.md`](ARCHITECTURE.md) — where code goes  
5. [`DECISIONS.md`](DECISIONS.md) — do not reverse without reason  

## 3. Smoke verify before coding

```bash
cargo test -p gust-core
cargo build -p gust

python3 -m http.server 8080   # terminal A

# terminal B
cargo run --release -p gust -- run http://localhost:8080/ --rate 100 --duration 5 --no-ui
cargo run --release -p gust -- run --scenario examples/journey.toml --rate 30 --duration 5 --no-ui
cargo run --release -p gust -- run http://localhost:8080/ --profile ramp --from 20 --to 200 --duration 8 --report /tmp/gust.html
```

## 4. Continue from here

**P0–P3 + compare/CI are done.** Prefer dogfooding + distribution polish
(GitHub Releases, demo GIF) before P4. Only start distributed work if one
machine is not enough. Recruiter loop: [`CASE-STUDY.md`](CASE-STUDY.md).

## 5. Do not

- Add Kafka “because distributed systems look good”
- Put HTTP/TUI logic into `gust-core`
- Drop raw-vs-corrected dual reporting
