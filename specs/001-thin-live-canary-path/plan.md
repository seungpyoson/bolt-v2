# Implementation Plan: Thin Bolt-v3 Live Canary Path

**Branch**: `001-thin-live-canary-path` | **Date**: 2026-05-13 | **Spec**: `specs/001-thin-live-canary-path/spec.md`
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
- Single path and config-controlled runtime: PASS for production entrypoint on current main; BLOCKED for Phase 6 until there is one live submit admission path consuming the validated gate report.
- Test-first safety gates: REQUIRED for all implementation slices; every code task in `tasks.md` has red/green verification.
- Evidence before claims: REQUIRED; no live-readiness claim without real SSM/venue artifact.
- Minimal slice discipline: REQUIRED; each numbered slice is a separate PR unless user explicitly approves combining doc-only planning with one code slice.

## Current Evidence From `main == origin/main` at `a5c60f2b6a4fe67fc80cf9d234f1512af09bec03`

- `git status --short --branch` returned `## main...origin/main`; `git rev-parse HEAD origin/main` returned `a5c60f2b6a4fe67fc80cf9d234f1512af09bec03` for both refs.
- `src/main.rs:56-57` builds through `build_bolt_v3_live_node(&loaded)?` and enters NT through `run_bolt_v3_live_node(&mut node, &loaded).await?`; production entrypoint adoption is present on main.
- `src/bolt_v3_live_node.rs:312-317` validates `BoltV3LiveCanaryGateReport` before runner entry, then drops the report; Phase 6 must make these validated bounds available to submit admission.
- `src/bolt_v3_live_node.rs:317-339` wires runtime capture and waits for capture shutdown around `node.run()`; Phase 6 must preserve this runner/capture behavior.
- `src/bolt_v3_live_node.rs:414-419` registers configured strategies during bolt-v3 build after client registration.
- `src/bolt_v3_live_canary_gate.rs:32-38` exposes the report fields Phase 6 must consume: approval id, readiness path, readiness byte cap, order-count cap, canary notional cap, and root notional cap.
- `src/bolt_v3_strategy_registration.rs:97-119` creates one `JsonlBoltV3DecisionEvidenceWriter` from the loaded config and passes cloned mandatory evidence handles into strategy registration contexts.
- `src/strategies/binary_oracle_edge_taker.rs:2825-2834` contains the only direct strategy NT submit helper; it records decision evidence before calling `self.submit_order(order, None, Some(client_id))`.
- `rg -n "bolt_v3_submit_admission|SubmitAdmission|admission" src tests` found no Phase 6 admission module or tests on main.
- `docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md:167-180` defines Bolt as thin over NT and NT as owner of runtime adapter behavior, market data, execution, and constructed-client behavior.
- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:193-195` defines NT Portfolio/cache-derived state as the source for account/position/order/fill facts.
- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:676-688` forbids a Bolt executable-order schema or venue translation layer and says adapter gaps should be fixed upstream in NT.
- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:1410-1418` defines PR #305 gate scope and states submit admission must separately consume validated bounds.

## Stale Phase 6 PR Audit

- PR #316 is a planning-only draft whose base is `010-bolt-v3-phase5-decision-evidence`, not current main. Its valid missing ideas are the ordering `decision evidence -> submit admission -> NT submit`, shared admission state, fail-closed count/notional checks, and evidence-failure-before-budget-consumption. Do not port the plan file wholesale: its line evidence and execution instructions are stale.
- PR #317 is an implementation draft whose base is `011-bolt-v3-phase6-submit-admission-plan`, not current main. Its valid missing pieces are an admission state module, focused tests, shared admission handle, gate-report arming, and strategy submit admission before NT submit. Do not merge, rebase, or port wholesale: its diff diverges from current main and its runner edits do not preserve the current runtime-capture shutdown path.
- Recommendation: close #316 and #317 as stale/superseded after user approval for GitHub mutation. Fresh Phase 6 implementation must start from current main after plan review.

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
└── operator_ignored_*.rs or an equivalent command harness
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

## Phase 6 Restart Plan

Scope:

- Add exactly one submit admission boundary for bolt-v3 live strategy submits.
- Consume only the validated `BoltV3LiveCanaryGateReport` produced by `check_bolt_v3_live_canary_gate`; do not duplicate live-canary TOML parsing in admission code.
- Enforce `max_live_order_count` as a global budget of admitted submit attempts across all registered strategies. Entry submits, exit submits, and replace-submit paths all consume the same budget. Plain cancel requests are not submits and do not consume budget.
- Consuming budget before NT submit is fail-closed and must not be refunded on NT submit error unless user approves a different requirement before implementation.
- Enforce `max_notional_per_order` against a strategy-supplied positive `Decimal` notional. For `BinaryOracleEdgeTaker`, notional is strategy-owned because the strategy knows the economic order shape.
- Preserve ordering: decision evidence persistence succeeds first, submit admission consumes budget second, NT `submit_order` happens third.
- Preserve `run_bolt_v3_live_node` runtime-capture behavior while making the validated gate report available to the shared admission state before runtime capture is wired and before the runner starts.
- Phase 6 inherits the existing readiness-report validation already performed by `check_bolt_v3_live_canary_gate`; it does not produce readiness evidence.

Public interface proposed:

```rust
use std::sync::{Arc, Mutex};

pub struct BoltV3LiveNodeRuntime {
    node: LiveNode,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
}

pub struct BoltV3SubmitAdmissionState;

impl BoltV3SubmitAdmissionState {
    pub fn new_unarmed() -> Self;
    pub fn arm(&self, report: BoltV3LiveCanaryGateReport) -> Result<(), BoltV3SubmitAdmissionError>;
    pub fn admit(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError>;
    pub fn admitted_order_count(&self) -> u32;
}

pub struct BoltV3SubmitAdmissionPermit(());

pub struct BoltV3SubmitAdmissionRequest {
    pub strategy_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: Decimal,
}

pub enum BoltV3SubmitAdmissionError {
    NotArmed,
    AlreadyArmed,
    CountCapExhausted,
    NonPositiveNotional,
    NotionalCapExceeded,
}
```

Integration sketch:

- `build_bolt_v3_live_node` and its test-only siblings return `BoltV3LiveNodeRuntime`, not a bare `LiveNode`.
- `build_live_node_with_clients` creates one `Arc<BoltV3SubmitAdmissionState>` before strategy registration and injects the same handle into `StrategyRegistrationContext` and `StrategyBuildContext`.
- `BoltV3LiveNodeRuntime` stays opaque: it does not expose the shared admission state and does not deref into raw `LiveNode`.
- `run_bolt_v3_live_node` accepts `&mut BoltV3LiveNodeRuntime`, calls `check_bolt_v3_live_canary_gate`, arms the runtime's internal admission state with that exact report, then preserves the existing runtime-capture and shutdown sequence around the internal `LiveNode::run()`.
- The admission `Arc` outlives `run_bolt_v3_live_node` so later Phase 8 evidence can inspect `admitted_order_count()` without adding lifecycle machinery.
- Admission state uses one internal mutex for gate report, armed flag, and count mutation. No mixed atomic/mutex state.
- Submit ordering remains strategy-enforced and source-fence-verified, not a new Bolt framework submit wrapper. The strategy-internal submit helper must require `BoltV3SubmitAdmissionPermit` before it calls NT `submit_order`.

Likely files touched:

- `src/bolt_v3_submit_admission.rs`
- `src/lib.rs`
- `src/bolt_v3_live_node.rs`
- `src/bolt_v3_strategy_registration.rs`
- `src/strategies/registry.rs`
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- `src/strategies/binary_oracle_edge_taker.rs`
- `tests/bolt_v3_submit_admission.rs`
- focused existing tests that construct strategy contexts or bolt-v3 live nodes

TDD sequence after approval:

1. Missing or unarmed gate report rejects before NT submit with `NotArmed`.
2. One-order cap allows first admission and rejects second.
3. Over notional cap rejects before NT submit; equality with the cap admits.
4. Evidence persistence failure rejects before admission budget consumption.
5. Successful admission ordering is evidence write, admission consume, NT submit.
6. Entry, exit, and replace-submit candidates all consume the same budget; cancel-only paths do not consume budget.
7. Double-arm and stale-arm behavior reject; a fresh `LiveNode` build creates a fresh unarmed admission state.
8. Source fences scan all strategy and archetype submit call sites for evidence, admission, permit, and NT-submit ordering.

Risks and unknowns:

- Notional semantics must stay strategy-owned; admission core must not learn Polymarket, Chainlink, or binary-oracle specifics.
- Admission state must be shared between registered strategies and the runner-arm path without cloning independent counters; no global singleton is allowed.
- Runtime-capture shutdown behavior in `run_bolt_v3_live_node` must survive the API change.
- Decision evidence records strategy intent. If admission later rejects, the JSONL intent exists without NT submit evidence; Phase 8 live proof must join against NT event evidence, not treat intent evidence as a submit.
- A process restart creates a fresh unarmed in-memory admission state. Phase 6 does not query NT cache to reconstruct prior admission count; Phase 8 operator procedure must avoid treating restart as budget preservation.
- Phase 7 no-submit readiness and Phase 8 live canary proof remain out of scope.

Stop conditions:

- Any design requiring Bolt-owned order lifecycle, reconciliation, adapter behavior, cache semantics, or venue translation.
- Any hardcoded runtime value, alternate secret source, alternate submit path, or duplicated config format.
- Any stale branch continuation or wholesale PR #316/#317 port.
- Any unresolved external review finding not accepted, disproved with current evidence, or explicitly deferred by user approval.
