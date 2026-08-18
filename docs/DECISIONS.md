# Gust — Decisions

Record of important choices so a later worktree does not re-litigate them without reason.

## D1 — Project: load-testing studio, not finance/Kafka demo

**Decision:** Build Gust (HTTP load + live degradation UI), not payment networks, bank sims, or portfolio risk labs.

**Why:** User wants real users + portfolio + learning; no prior finance experience; cost $0; Kafka optional. Load testing is a place Rust is *required* (tool must not be the bottleneck) and design craft is the moat.

**Rejected:** Market microstructure lab (user already built), payment Kafka backbone (no users), bank game (user disliked), CS visualizer farms (saturated 2026).

## D2 — Name: Gust

**Decision:** Product name = **Gust**.

**Why:** Short, memorable, one syllable; wind-burst metaphor for sudden load; CLI-friendly (`gust run`). Crest/Plimsoll/Loadmark rejected (long, crowded, or hard to remember). Thrash rejected as less clean.

**Note:** Spot-check crates.io/GitHub before `cargo publish`; name collisions elsewhere are OK if not in load-testing.

## D3 — Open model + coordinated omission first

**Decision:** Day-one correctness is open-model scheduling + HDR coordinated-omission correction; always show raw vs corrected.

**Why:** This is the classic lie load testers tell. Teaching/showing the gap is the product’s intellectual core and first blog post.

**Reference:** Gil Tene — coordinated omission.

## D4 — Split `gust-core` vs `gust-cli`

**Decision:** Pure measurement library separate from I/O binary.

**Why:** CO logic is subtle; must be unit-tested without network. Keeps knee detection (P2) testable the same way.

## D5 — TUI before HTML / desktop GUI

**Decision:** P1 = ratatui in-terminal dashboard; HTML report in P2; native desktop GUI later if ever.

**Why:** Ship craft fast with zero install friction beyond the binary. HTML is the shareable artifact for Show HN / Slack. Competing with loadr’s desktop GUI is not the first battle.

## D6 — Differentiate from loadr

**Decision:** Position around **finding and explaining the breaking point (knee)**, not “all of k6/JMeter/Gatling in one Rust binary.”

**Why:** loadr.io already claims that broad space. Gust’s sharper story is safer.

## D7 — No Kafka until P4 need is real

**Decision:** Do not introduce Kafka/Redis/Redpanda in P0–P3.

**Why:** User wanted Rust deep; Kafka was optional. Premature distributed infra fights the $0 / local-first goal.

## D8 — HTTP GET first; scenarios later

**Decision:** P0/P1 = single GET URL only.

**Why:** Unblocks correctness + UI without drowning in protocol surface. Multi-step scenarios = P3.

## D9 — Unbounded sample channel for now

**Decision:** `mpsc::unbounded_channel` for Sample → recorder.

**Why:** Simple; recorder drains aggressively. Revisit if generator outpaces recorder at extreme RPS (document drop policy then).

## D10 — Color / UI language

**Decision (soft):** p50 green, p90 yellow, p99 light red; cyan brand chip for “gust”; red highlight when corrected ≫ raw.

**Why:** Instant reading of “tail is peeling away.” Avoid purple-gradient AI-slop look; keep terminal craft tight.

## D11 — crates.io package name is not `gust`

**Decision:** Keep the binary and GitHub repo named **Gust**, but do **not**
publish a crates.io package as `gust`. That name is already taken by an
unrelated 2017 charting library (saresend/Gust). If we publish later, use a
distinct package name (candidate: `gust-load`) and keep `[[bin]] name = "gust"`.

**Why:** Spot-check before publish was already noted in D2; confirmed taken
2026-08. Renaming the CLI would throw away the brand for no user benefit.

## D12 — Regression artifacts over more protocols

**Decision:** After P0–P3, prioritize JSON run artifacts, `gust compare`, and CI
thresholds over new protocols (gRPC/WS) or distributed generators.

**Why:** Recruiter / hiring-manager value is "I measured a system, fixed it, and
proved the capacity change." Compare + gates make that loop crisp. Extra
protocols broaden surface without sharpening the story; P4 stays need-driven.

**See:** [`CASE-STUDY.md`](CASE-STUDY.md).

