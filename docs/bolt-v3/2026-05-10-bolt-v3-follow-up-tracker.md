# Bolt-v3 Follow-Up Tracker

Date: 2026-05-10
Branch context: `codex/bolt-v3-reconciliation`

This tracker separates accepted local idle-tracer work from remaining bolt-v3 production gates.
It is not a broad roadmap. Each item should become one narrow issue or PR only when selected.

## Status Legend

- `unverified`: not proven in current branch
- `verified-local`: proven by local tests in current branch, not production evidence
- `reserved`: accepted concept, intentionally unsupported now
- `blocked`: needs prior decision or evidence

## Follow-Ups

| ID | Item | Status | Required Proof | Non-Goals |
| --- | --- | --- | --- | --- |
| F1 | Multi-strategy idle verification | verified-local | `tests/bolt_v3_strategy_registration.rs` proves root TOML with 2 `strategy_files` builds one `LiveNode`, registers both strategy IDs from TOML, reaches `NodeState::Idle`, and source-fences registration wiring from submit/subscribe calls | live run, order lifecycle, many-client scale |
| F2 | Reference role naming | blocked | Decide whether `reference_data.primary` is correct role language; update schema/tests only after decision | fused-price policy, producer wiring |
| F3 | ETH/USD reference contract | blocked | Define logical reference stream name, required inputs, freshness/confidence semantics, and source ownership; current eth tracer has only `parameters.reference_publish_topic`, while `[reference_data]` is not present in the eth strategy fixture | live orders, order admission |
| F4 | Fused-price policy | blocked | Tests for anchor source, fast-feed modifiers, weights, stale handling, disagreement handling, fail-closed cases | client implementation |
| F5 | Reference producer wiring | blocked | v3 TOML must create a producer path and the strategy must receive `ReferenceSnapshot` on the configured topic; current producer code is legacy `Config.reference`/`ReferenceActor`, not bolt-v3 root/strategy TOML | changing strategy signal logic |
| F6a | Instrument selected-market cache resolution | verified-local | `tests/bolt_v3_instrument_readiness.rs` proves v3 TOML strategy targets plan into per-client cache checks; loaded NT `BinaryOption` instruments resolve one `selected_market`; stale time windows fail closed; missing target instruments remain blocked | `request_instruments`, reference price facts, strategy activation, submit/cancel/fill |
| F6b | Instrument readiness gate integration | verified-local | `tests/bolt_v3_instrument_gate.rs` proves a built bolt-v3 `LiveNode` stays `NodeState::Idle` while a pre-start gate reports `Blocked` for missing selected-market instruments and `Ready` for loaded selected-market instruments from NT cache | `request_instruments`, automatic start/run enforcement, submit/cancel/fill |
| F6c | Instrument load through NT startup sequence | blocked | NT v1.226.0 `LiveNode::start` performs data-client connect, private pending-data flush, execution-client connect, reconciliation, then trader start. bolt-v3 needs a public NT-aligned pre-trader load/flush hook or an accepted start gate that checks readiness before trader activation | direct provider fetch into cache, reimplementing NT runner flush, Python, submit/cancel/fill |
| F7 | Decision-event persistence | blocked | Durable event before submit intent, with config IDs, target facts, reference facts, computed decision, and no-action reasons. Current fixed nullable-field contract conflicts with pinned NT `#[custom_data]`, which rejects `Option<String>` fields | sentinel null encoding, JSON blob fallback, second writer path |
| F8 | Risk/order admission gate | blocked | Config-driven size/exposure/cooldown/kill-switch check before any order reaches NT execution. v3 maps NT `LiveRiskEngineConfig`, but bolt-owned order admission needs an accepted decision-event persistence gate and a v3 order intent path | venue-specific order lifecycle, ad hoc strategy-local submit guards |
| F9 | Order lifecycle proof | blocked | Submit/cancel/fill/reject path proven through NT for one venue under controlled conditions. NT and the legacy strategy have order machinery, but bolt-v3 has not proven a v3 run path with accepted decision evidence and bolt-owned order admission | direct venue calls, Python, bypassing NT risk/execution, multi-client scale |
| F10 | Reconciliation/restart proof | blocked | Restart observes external orders/fills/positions and avoids duplicate submit. bolt-v3 maps NT reconciliation config, but no v3 start/restart proof exists and the pinned Polymarket adapter's external-order registration hook is a no-op | bolt-owned portfolio, direct venue reconciliation, new strategy logic |
| F11 | Fixed-instrument target | reserved | Schema, validation, planner, event facts, and idle test for `market_selection_type = "fixed_instrument"` | rotating-market refactor |
| F12 | Scale/process model | unverified | Evidence for many strategies, markets, and clients; process sharding; panic behavior; restart discipline | changing trading logic |

## Current Narrow Proof

The current branch aims to prove only:

```text
v3 TOML -> registered idle LiveNode -> NT cache selected-market readiness gate -> Ready/Blocked before start
```

This does not prove production readiness, live orders, automatic instrument loading, fused-reference correctness, automatic start/run enforcement, decision persistence, order lifecycle, reconciliation, or scale.

## Current Blocker

F6c is blocked because the accepted NT path that populates the cache before execution clients is inside `LiveNode::start`. That path also runs reconciliation and starts the trader. NT source evidence at pinned `nautilus_trader@38b912a`: `crates/live/src/node.rs` startup sequencing comments, `LiveNode::start`, and private `flush_pending_data`; `crates/live/src/runner.rs` routes `DataEvent::Instrument` into `DataEngine`; `crates/data/src/engine/mod.rs` processes `InstrumentAny` into cache.

Do not work around this by fetching venue instruments directly in bolt-v3 and writing them into the NT cache. That creates a second production instrument-loading path.

## Reference Blocker

F3-F5 are blocked by a contract gap, not code volume. Current bolt-v3 eth tracer passes `parameters.reference_publish_topic` into the existing strategy. It does not define reference inputs, source ownership, weights, freshness, or producer construction from bolt-v3 TOML. Existing `ReferenceActor` proves a legacy producer exists under old `Config.reference`, but wiring that into bolt-v3 without a v3 contract would create a hidden dual config path.

Do not create a bolt-v3 reference producer until the v3 reference contract names the configured stream, its input sources, freshness rules, and whether `reference_data` remains in strategy TOML or moves to root-level reference config.

## Decision-Event Blocker

F7 is blocked by an NT serialization contract mismatch. A TDD probe for a production `BoltV3MarketSelectionDecisionEvent` custom-data type failed because pinned NT `#[custom_data]` does not support nullable `Option<String>` fields. The current runtime contract requires nullable event fields to be explicit nulls. Replacing nulls with sentinel strings or hiding the event in an ad hoc JSON blob would violate the contract and create forensic ambiguity.

Accepted unblocker: revise the decision-event contract to match pinned NT custom-data field support, or prove a pinned NT custom-data encoding that preserves explicit null semantics without a second persistence path.

## Risk Gate Blocker

F8 is blocked above the NT RiskEngine. Pinned NT RiskEngine provides pre-trade checks (`nautilus_trader@38b912a:crates/risk/src/engine/mod.rs:81`, `:721`, `:777`), and bolt-v3 already maps root TOML into `LiveRiskEngineConfig` (`src/bolt_v3_live_node.rs:425`) with `nt_bypass = false` validation (`src/bolt_v3_validate.rs:271`). That is necessary but not sufficient for bolt-owned admission: size, exposure, cooldown, kill-switch, and no-action evidence must be evaluated before any order reaches NT execution.

Do not add one-off submit guards inside `eth_chainlink_taker` as the production solution. The accepted gate must sit on the v3 order-intent path after decision evidence is accepted and before NT order submission.

## Order Lifecycle Blocker

F9 is blocked by missing bolt-v3 order-intent evidence, not by missing NT primitives. The existing strategy can construct and submit NT orders and handle order events (`src/strategies/eth_chainlink_taker.rs:2827`, `:2978`, `:3324`, `:3410`, `:3435`). Pinned NT routes submitted orders through RiskEngine to ExecutionEngine (`nautilus_trader@38b912a:crates/risk/src/engine/mod.rs:496`, `:1764`; `crates/execution/src/engine/mod.rs:1625`), and the pinned Polymarket adapter exposes submit/cancel methods (`crates/adapters/polymarket/src/execution/mod.rs:792`, `:1036`).

Bolt-v3 only proves build, client registration, strategy registration, and idle/controlled-connect behavior today. `build_live_node_with_clients` builds the node and registers strategies (`src/bolt_v3_live_node.rs:298`), while current v3 tests intentionally fence out `LiveNode::start` and `LiveNode::run`. An accepted order-lifecycle slice must come after F6c, F7, and F8: instrument load/start gate, durable decision event contract, and bolt-owned order admission.

## Reconciliation / Restart Blocker

F10 must stay NT-owned. Bolt-v3 already maps TOML into NT reconciliation config (`src/bolt_v3_live_node.rs:371`, `:381`), and pinned NT startup reconciliation queries execution mass status, reconciles it, and registers external orders (`nautilus_trader@38b912a:crates/live/src/node.rs:486`, `:532`, `:547`, `:569`). Pinned NT reconciliation also deduplicates already-synced or closed reconciliation orders (`crates/live/src/manager.rs:354`, `:396`, `:407`, `:425`).

That is not bolt-v3 restart readiness. Current bolt-v3 proof stops before `LiveNode::start` / `LiveNode::run`, so no restart path has observed venue orders, fills, balances, positions, or duplicate-submit behavior under the v3 runtime. For Polymarket specifically, the adapter can generate mass status (`crates/adapters/polymarket/src/execution/mod.rs:1513`), but `register_external_order` is currently empty (`:1283`), so external-order tracking after reconciliation needs source or canary evidence before acceptance.

Accepted unblocker: after F6c, F7, F8, and F9, prove one NT-owned restart scenario end to end under bolt-v3: start, submit or import one known order state, stop/crash boundary, restart, startup reconciliation, no duplicate submit, and correct order/fill/position state in NT.
