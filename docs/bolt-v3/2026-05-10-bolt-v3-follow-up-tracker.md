# Bolt-v3 Follow-Up Tracker

Date: 2026-05-10
Branch context: `codex/bolt-v3-strategy-idle`

This tracker separates accepted local idle-tracer work from remaining bolt-v3 production gates.
It is not a broad roadmap. Each item should become one narrow issue or PR only when selected.

## Status Legend

- `unverified`: not proven in current branch
- `reserved`: accepted concept, intentionally unsupported now
- `blocked`: needs prior decision or evidence

## Follow-Ups

| ID | Item | Status | Required Proof | Non-Goals |
| --- | --- | --- | --- | --- |
| F1 | Multi-strategy idle verification | unverified | Root TOML with 2+ `strategy_files` builds one `LiveNode`, registers all strategies from TOML, reaches `NodeState::Idle`, no submit/subscribe calls in registration wiring | live run, order lifecycle, many-client scale |
| F2 | Reference role naming | blocked | Decide whether `reference_data.primary` is correct role language; update schema/tests only after decision | fused-price policy, producer wiring |
| F3 | ETH/USD reference contract | blocked | Define logical reference stream name, required inputs, freshness/confidence semantics, and source ownership | live orders, order admission |
| F4 | Fused-price policy | blocked | Tests for anchor source, fast-feed modifiers, weights, stale handling, disagreement handling, fail-closed cases | client implementation |
| F5 | Reference producer wiring | unverified | TOML creates reference producer path; strategy receives `ReferenceSnapshot` on configured topic | changing strategy signal logic |
| F6 | Instrument readiness gate | unverified | Selected market instruments are loaded/known in NT before strategy can proceed past readiness | submit/cancel/fill |
| F7 | Decision-event persistence | unverified | Durable event before submit intent, with config IDs, target facts, reference facts, computed decision, and no-action reasons | full event lake design rewrite |
| F8 | Risk/order admission gate | unverified | Config-driven size/exposure/cooldown/kill-switch check before any order reaches NT execution | venue-specific order lifecycle |
| F9 | Order lifecycle proof | unverified | Submit/cancel/fill/reject path proven through NT for one venue under controlled conditions | multi-client scale |
| F10 | Reconciliation/restart proof | unverified | Restart observes external orders/fills/positions and avoids duplicate submit | new strategy logic |
| F11 | Fixed-instrument target | reserved | Schema, validation, planner, event facts, and idle test for `market_selection_type = "fixed_instrument"` | rotating-market refactor |
| F12 | Scale/process model | unverified | Evidence for many strategies, markets, and clients; process sharding; panic behavior; restart discipline | changing trading logic |

## Current Narrow Proof

The current branch aims to prove only:

```text
v3 TOML -> existing strategy -> NT Strategy -> LiveNode.add_strategy -> NodeState::Idle
```

This does not prove production readiness, live orders, fused-reference correctness, instrument readiness, decision persistence, order lifecycle, reconciliation, or scale.
