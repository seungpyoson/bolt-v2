# Feature Specification: Thin Bolt-v3 Live Canary Path

**Feature Branch**: `001-thin-live-canary-path`
**Created**: 2026-05-12
**Status**: Locked for Phase 1
**Input**: User description: define the problem with hard evidence, plan before runtime code, then TDD the 1-8 path into a production-shaped bolt-v3 spine proven with tiny capital. Initial strategy is a taker strategy comparing Polymarket option price against Chainlink and configurable exchange references using option-pricing edge. Core must not be hardcoded to Binance, Chainlink, Polymarket, one market family, or one strategy.

## User Scenarios & Testing

### User Story 1 - Production Enters One Bolt-v3 Path (Priority: P1)

As the operator, I can run the production binary through the bolt-v3 config, secret, adapter, strategy, gate, and NT runner path without the legacy runtime path remaining available.

**Why this priority**: Without one production entrypoint, every other proof can be bypassed by the binary actually being run.

**Independent Test**: A source and integration test proves `src/main.rs` uses the bolt-v3 load/validate/build/run path and contains no direct production `LiveNode::run` or legacy ruleset runtime construction path.

**Acceptance Scenarios**:

1. **Given** a valid bolt-v3 TOML, **When** the production binary starts, **Then** it loads and validates `LoadedBoltV3Config`, resolves SSM secrets, maps provider adapters, registers NT clients, registers strategies, and enters NT only through `run_bolt_v3_live_node`.
2. **Given** an invalid or missing bolt-v3 TOML section, **When** the production binary starts, **Then** startup fails closed before NT runner entry.
3. **Given** a contributor adds a second production runner path, **When** verification runs, **Then** the runner-fence test fails before merge.

---

### User Story 2 - Submit Requires One Admission Path (Priority: P1)

As the operator, I can trust that every live order candidate passes through one submit admission gate that consumes the live canary report bounds and mandatory decision evidence.

**Why this priority**: A capped live canary is unsafe if caps and evidence are validated at startup but not enforced at submit.

**Independent Test**: Unit and integration tests prove orders are rejected when the validated gate report is absent, order count is exhausted, notional exceeds config cap, decision evidence cannot be persisted, or strategy evidence is missing.

**Acceptance Scenarios**:

1. **Given** `max_live_order_count = 1`, **When** one order has already been admitted, **Then** the second live order candidate is rejected before calling NT submit.
2. **Given** `max_notional_per_order = "1.00"`, **When** a strategy proposes a larger live order, **Then** submit admission rejects it before calling NT submit.
3. **Given** missing decision evidence, **When** a strategy is constructed or tries to submit, **Then** construction or admission fails closed; no fallback submit path exists.

---

### User Story 3 - Initial Taker Strategy Runs Through Generic Registries (Priority: P1)

As the operator, I can configure an initial binary-oracle edge taker that compares Polymarket option prices against a fair probability derived from Chainlink and configurable exchange references, without hardcoding the core to Polymarket, Chainlink, Binance, or one market family.

**Why this priority**: The canary must prove the final generic production shape, not a one-off Polymarket script.

**Independent Test**: Registry tests inject fake providers, fake market families, and fake strategy bindings to prove core dispatch does not name the concrete venue, market family, or strategy. Strategy tests prove its decision parameters and reference roles are TOML-driven.

**Acceptance Scenarios**:

1. **Given** a TOML strategy archetype and reference-data roles, **When** validation runs, **Then** the strategy binding validates required roles and parameters without core deserializing concrete strategy internals.
2. **Given** multiple configured exchange reference venues, **When** the strategy evaluates an entry, **Then** it uses only configured reference roles and NT/cache-derived facts.
3. **Given** an unsupported provider or market family, **When** validation runs, **Then** it fails closed with a provider-owned or family-owned diagnostic before runtime.

---

### User Story 4 - Real No-submit Readiness Produces Gate Evidence (Priority: P2)

As the operator, I can run authenticated no-submit readiness against real SSM and real venue connectivity, produce a redacted readiness report, and have the PR #305 gate consume that report before runner entry.

**Why this priority**: The live canary gate is useful only after it consumes real readiness evidence.

**Independent Test**: An ignored operator-run test or CLI path connects and disconnects through NT using real SSM/venue config, submits zero orders, writes a redacted report, and then the gate accepts that report.

**Acceptance Scenarios**:

1. **Given** real SSM paths and venue config, **When** no-submit readiness runs, **Then** it connects/disconnects through NT and writes a report with all required stages satisfied.
2. **Given** an unsatisfied or stale readiness report, **When** production startup reaches the live canary gate, **Then** the gate rejects before NT runner entry.

---

### User Story 5 - Tiny-capital Canary Proves the Spine (Priority: P2)

As the operator, I can approve one tiny live order with configured caps and get evidence for submit, accept/fill/reject, strategy-driven cancel if open, and restart reconciliation through NT.

**Why this priority**: This is the first proof that bolt-v3 is a running production-shaped system rather than local scaffolding.

**Independent Test**: Operator-run canary artifact includes exact config checksum, SSM paths, manifest hash, order facts, NT event evidence, and restart reconciliation evidence. Local tests cover all fail-closed preconditions; live artifact proves venue path.

**Acceptance Scenarios**:

1. **Given** all prior gates pass and explicit approval is recorded, **When** the canary runs, **Then** at most one configured tiny order is submitted through NT.
2. **Given** the order remains open after the acceptance window, **When** the strategy exit/cancel path runs, **Then** cancellation is strategy-driven through NT, not exec-engine-direct test machinery.
3. **Given** the process restarts after canary order activity, **When** reconciliation runs, **Then** NT adapter state imports venue-confirmed order/fill status without duplicate submit.

## Edge Cases

- Legacy `src/main.rs` runner path remains present after bolt-v3 entrypoint adoption.
- Valid startup gate report is accepted but submit-time caps are not consumed.
- Strategy registration exists for one concrete strategy by hardcoding core instead of using registry bindings.
- Exchange reference venue count grows but core requires code edits for each provider.
- Real venue adapter lacks capability required by the canary; preferred fix is upstream NT adapter or explicit blocker, not Bolt-side adapter reimplementation.
- no-submit readiness succeeds locally but report cannot be consumed by the PR #305 gate.
- Live canary order is accepted but never fills; strategy-driven cancel and restart reconciliation must still be proven.

## Requirements

### Functional Requirements

- **FR-001**: The production binary MUST have one bolt-v3 build/run path from TOML load to NT runner entry.
- **FR-002**: Production code MUST NOT call `LiveNode::run` directly for bolt-v3; it MUST call `run_bolt_v3_live_node`.
- **FR-003**: The legacy config/ruleset/runtime path MUST be removed or made unreachable from production before tiny-capital live submit.
- **FR-004**: Every runtime parameter MUST come from TOML and every secret MUST resolve from AWS SSM through Rust SDK code.
- **FR-005**: Core build, secret, adapter, market-family, strategy, and admission logic MUST remain venue-, market-, and strategy-agnostic.
- **FR-006**: Concrete providers, market families, and strategies MUST be selected through registries or binding tables, not core matches that require core edits per new concrete type.
- **FR-007**: The first registered strategy binding MUST be a taker strategy archetype for binary oracle markets, configured through execution venue, primary oracle reference, and optional exchange reference roles selected by TOML and registries; core architecture MUST NOT require Polymarket, Chainlink, Binance, or any specific market family.
- **FR-008**: The strategy binding MUST accept multiple configured exchange reference venues through roles/config; it MUST NOT be hardcoded to one exchange.
- **FR-009**: Submit admission MUST consume `BoltV3LiveCanaryGateReport.max_live_order_count` and `max_notional_per_order` before every live submit.
- **FR-010**: Strategy construction or submit admission MUST fail closed when mandatory bolt-v3 decision evidence is absent or cannot be persisted.
- **FR-011**: Bolt-v3 MUST NOT own order lifecycle, reconciliation, adapter behavior, NT cache semantics, or local mock venue worlds as live-readiness proof.
- **FR-012**: Authenticated no-submit readiness MUST require explicit operator approval and produce a redacted report from real SSM and venue connectivity before any tiny-capital submit.
- **FR-013**: Tiny-capital canary MUST be explicitly approved, cap-enforced, single-path, and proven through NT adapter submit, accept/fill/reject, strategy-driven cancel if needed, and restart reconciliation.
- **FR-014**: Every implementation slice MUST use TDD red-green-refactor and `superpowers:verification-before-completion` before phase completion.
- **FR-015**: no-mistakes status MUST be captured for task triage or branch gating when the tool is available; any issue-specific binary override belongs in quickstart/operator notes, not this durable system requirement.
- **FR-016**: Backtesting and research analytics MUST remain out of this MVP unless required to prove canary safety; they require a separate spec.

### Key Entities

- **BoltV3RuntimeConfig**: TOML-backed loaded root config plus strategy files, venue blocks, risk settings, live canary block, persistence settings, and NT config mapping.
- **ProviderBinding**: Provider-owned validation, secret resolution, adapter mapping, credential log filters, and supported market-family declaration.
- **MarketFamilyBinding**: Market-family-owned validation, instrument trait requirements, and supported-provider compatibility declaration.
- **StrategyBinding**: Strategy-owned validation, build, decision policy, reference-role declaration, and evidence requirement.
- **BoltV3LiveCanaryGateReport**: Validated operator approval, readiness report path, readiness report byte cap, order count cap, notional cap, and root cap.
- **SubmitAdmissionState**: Runtime counter and cap state consumed before each live submit.
- **NoSubmitReadinessReport**: Redacted real-connectivity report consumed by the live canary gate.
- **CanaryRunEvidence**: Redacted artifact proving config checksum, approval, NT submit, venue result, cancel if needed, and reconciliation.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Exact-head verification shows `src/main.rs` no longer contains a production direct `node.run()` path outside the bolt-v3 wrapper.
- **SC-002**: Tests fail before implementation and pass after implementation for each of the 1-8 slices.
- **SC-003**: Source-fence tests prove core files do not name concrete provider, market-family, or strategy keys outside approved binding modules.
- **SC-004**: Submit-admission tests prove a second canary order and an over-cap notional never call NT submit.
- **SC-005**: A real no-submit readiness report is generated from real SSM/venue connectivity and accepted by the live canary gate.
- **SC-006**: Tiny-capital canary artifact proves at most one live order, configured notional cap, NT adapter submit, venue accept/fill/reject, strategy-driven cancel if open, and restart reconciliation.

## Assumptions

- PR #305 is merged into `origin/main` and provides the current `run_bolt_v3_live_node` fail-closed gate.
- The MVP can start with one execution venue and multiple configured reference venues, but the core architecture must allow more providers without core rewrites.
- If an NT adapter lacks required live capability, the blocker is recorded and the preferred fix is in NT adapter code, not a Bolt-side reimplementation.
- Real SSM and venue operations require explicit operator approval outside local test runs.
