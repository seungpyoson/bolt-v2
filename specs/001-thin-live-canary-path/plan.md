# Implementation Plan: Thin Bolt-v3 Live Canary Path

**Branch**: `001-thin-live-canary-path` | **Date**: 2026-05-12 | **Spec**: `specs/001-thin-live-canary-path/spec.md`
**Input**: Feature specification from `/specs/001-thin-live-canary-path/spec.md`

## Summary

Build the shortest production-shaped bolt-v3 spine that can safely prove one tiny live canary through NautilusTrader. The implementation path is eight narrow TDD slices: governance/evidence, production entrypoint adoption, generic registries, initial binary-oracle taker strategy activation, mandatory decision evidence, submit admission cap consumption, authenticated no-submit readiness, and tiny-capital canary proof.

The central constraint is that canary mode is not a separate architecture. It is the final bolt-v3 production path run with tiny TOML caps and explicit operator approval.

## Technical Context

**Language/Version**: Rust, current workspace toolchain
**Primary Dependencies**: NautilusTrader Rust crates, AWS Rust SDK for SSM, existing TOML/serde stack
**Storage**: Existing NT catalog/runtime capture surfaces plus redacted JSON artifacts for readiness/canary evidence
**Testing**: `cargo test`, `cargo fmt`, `cargo clippy`, targeted integration tests, exact-head CI, no-mistakes triage
**Target Platform**: Local/operator production Rust binary
**Project Type**: Rust live trading binary over NautilusTrader
**Performance Goals**: No new hot-path framework; admission checks must be constant-time against in-memory validated config/counters
**Constraints**: no hardcodes, no dual paths, SSM-only secrets, pure Rust binary, NT owns lifecycle/cache/reconciliation/adapters
**Scale/Scope**: MVP proves one tiny live canary. Generalized production trading expands via config, registries, provider bindings, and evidence per venue path, not core rewrites.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- NT-first thin layer: PASS for planned scope; no Bolt-side order lifecycle or reconciliation components are allowed.
- Generic core: PASS only if concrete provider/market/strategy additions stay in binding modules and source fences prove no core leakage.
- Single path and config-controlled runtime: BLOCKED until `src/main.rs` adopts bolt-v3 and legacy production run path is removed.
- Test-first safety gates: REQUIRED for all implementation slices; every code task in `tasks.md` has red/green verification.
- Evidence before claims: REQUIRED; no live-readiness claim without real SSM/venue artifact.
- Minimal slice discipline: REQUIRED; each numbered slice is a separate PR unless user explicitly approves combining doc-only planning with one code slice.

## Current Evidence From `origin/main` at `4f91409908d7b36903abc3edd555f93ae0646484`

- `src/main.rs:240-242` still creates `run_future = node.run()` directly. Production entrypoint adoption is not complete.
- `src/bolt_v3_live_node.rs:240-258` builds a bolt-v3 `LiveNode` from loaded config, SSM secrets, adapter mapping, and NT client registration.
- `src/bolt_v3_live_node.rs:268-277` gates `run_bolt_v3_live_node` before `node.run().await`, but the gate report is currently only validated and dropped.
- `src/bolt_v3_live_node.rs:1-35` explicitly states the bolt-v3 build path does not register strategies, construct orders, or enable submit paths.
- `src/bolt_v3_providers/mod.rs:111-132` currently registers only Polymarket and Binance provider bindings.
- `src/bolt_v3_archetypes/mod.rs:34-37` currently registers only `binary_oracle_edge_taker` for validation.
- `docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md:167-180` defines Bolt as thin over NT and NT as owner of runtime adapter behavior, market data, execution, and constructed-client behavior.
- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:193-195` defines NT Portfolio/cache-derived state as the source for account/position/order/fill facts.
- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:676-688` forbids a Bolt executable-order schema or venue translation layer and says adapter gaps should be fixed upstream in NT.
- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:1410-1418` defines PR #305 gate scope and states submit admission must separately consume validated bounds.

## Project Structure

### Documentation (this feature)

```text
specs/001-thin-live-canary-path/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── tasks.md
└── contracts/
    ├── live-canary-gates.md
    ├── thin-boundary.md
    └── review-gates.md
```

### Source Code Target Layout

```text
src/
├── main.rs
├── bolt_v3_config.rs
├── bolt_v3_live_node.rs
├── bolt_v3_live_canary_gate.rs
├── bolt_v3_submit_admission.rs
├── bolt_v3_strategy_registration.rs
├── bolt_v3_providers/
├── bolt_v3_market_families/
├── bolt_v3_archetypes/
└── strategies/<registered_strategy_module>.rs

tests/
├── bolt_v3_production_entrypoint.rs
├── bolt_v3_submit_admission.rs
├── bolt_v3_strategy_registration.rs
├── bolt_v3_provider_binding.rs
├── bolt_v3_live_canary_gate.rs
└── operator_ignored_live_canary.rs
```

**Structure Decision**: Keep core generic modules small and registry-driven. Concrete provider/market/strategy behavior stays in provider, market-family, archetype, or strategy modules. `src/main.rs` becomes a thin production entrypoint and stops owning legacy runtime assembly.

## Eight-slice Plan

1. **Evidence and contracts**: land this SpecKit branch with constitution, spec, plan, contracts, and tasks. No Rust behavior change.
2. **Production entrypoint adoption**: make `src/main.rs` call the tested bolt-v3 load/validate/build/run path. No live submit.
3. **Generic strategy/runtime registration**: add bolt-v3 strategy registration into the live-node build path without concrete strategy leakage into core.
4. **Initial taker strategy activation**: wire the existing binary-oracle edge taker through config roles for Polymarket option venue, Chainlink primary reference, and configurable exchange references.
5. **Mandatory decision evidence**: remove optional/fallback submit behavior; construction/admission fails closed without evidence.
6. **Submit admission**: consume `BoltV3LiveCanaryGateReport` at submit boundary and enforce order count and per-order notional.
7. **Authenticated no-submit readiness**: run real SSM/venue connect/disconnect, zero orders, redacted report consumed by PR #305 gate.
8. **Tiny-capital canary**: one approved capped live order through NT, with accept/fill/reject, strategy-driven cancel if open, and restart reconciliation evidence.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| None accepted | N/A | Any new framework must be rejected unless a TDD slice proves existing NT/config/registry surfaces cannot satisfy the requirement |

## Phase Gates

- **Gate A**: Docs/spec branch only; `git diff --check`, placeholder scan, no-mistakes status/runs captured.
- **Gate B**: Each code slice has red test output, green test output, relevant broader verification, and no-mistakes triage before review.
- **Gate C**: External review request only after clean branch, pushed commits, exact-head CI green, no known unresolved findings.
- **Gate D**: Live no-submit and tiny-capital runs require explicit operator approval and redacted artifacts.
