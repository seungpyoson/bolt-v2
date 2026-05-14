# Feature Specification: Bolt-v3 Phase 8 Tiny-capital Canary Machinery

**Feature Branch**: `018-bolt-v3-phase8-canary-readiness-fresh`
**Created**: 2026-05-14
**Status**: Draft
**Input**: User request for fresh-main Phase 8 tiny-capital canary machinery, excluding actual live order unless explicitly approved at runtime.

## User Scenarios & Testing

### User Story 1 - Block Unsafe Canary Start (Priority: P1)

As the operator, I can run local Phase 8 precondition checks that fail closed before any NT runner entry or live submit when required Phase 7 no-submit evidence, live canary caps, strategy-input approval, or exact operator approval is missing.

**Why this priority**: One tiny live order is still real capital. The first deliverable must prove the canary cannot start from stale branch work, stale readiness evidence, missing caps, or unreviewed strategy math.

**Independent Test**: Local behavior tests verify every missing or invalid precondition produces a blocked canary evidence artifact and never calls NT submit APIs.

**Acceptance Scenarios**:

1. **Given** current main lacks Phase 7 no-submit readiness producer files, **When** Phase 8 preflight evaluates canary readiness, **Then** it blocks and records the missing Phase 7 dependency.
2. **Given** `[live_canary]` has missing or invalid caps, **When** preflight evaluates canary readiness, **Then** it blocks before `run_bolt_v3_live_node`.
3. **Given** strategy-input safety audit status is `blocked`, **When** an operator tries the canary harness, **Then** the harness exits before build/run and writes blocked evidence only.

---

### User Story 2 - Produce Dry Canary Evidence (Priority: P2)

As the operator, I can produce a redacted dry/no-submit canary proof that joins decision intent, live canary gate result, submit admission state, no-submit readiness status, runtime capture capability, and explicit stop reason without placing an order.

**Why this priority**: Phase 8 must prove the live-capital path shape before any live order approval. Dry proof is the mandatory bridge from Phase 7 to live order readiness.

**Independent Test**: A local test creates a fixture canary evidence artifact and verifies it contains only redacted config identity, hashes, caps, preflight status, and blocked/no-submit outcome.

**Acceptance Scenarios**:

1. **Given** an approved no-submit readiness report fixture and tiny caps, **When** dry canary evidence is written, **Then** it records the report path hash, root config checksum, approval id hash, cap values, `outcome = dry_no_submit_proof`, and `block_reasons` containing `blocked_before_live_order`.
2. **Given** decision evidence writing fails, **When** dry proof runs, **Then** it records `outcome = blocked_before_submit` with `block_reasons` containing `decision_evidence_unavailable` and blocks before submit admission.

---

### User Story 3 - Prepare One-order Operator Harness (Priority: P3)

As the operator, I can inspect an ignored operator harness that can run the production bolt-v3 path with one configured tiny live order only after exact command, exact head SHA, approved config checksum, approved SSM manifest hash, and runtime approval id are supplied.

**Why this priority**: The live-capital command must be ready for review, but it must remain inert by default and must not run without explicit runtime approval.

**Independent Test**: Default test run reports the operator harness ignored; source fences prove the harness uses `build_bolt_v3_live_node` and `run_bolt_v3_live_node` within a `tokio::task::LocalSet` context only, never direct `LiveNode::run`, manual submit, manual cancel, or Bolt-owned reconciliation.

**Acceptance Scenarios**:

1. **Given** default `cargo test`, **When** the Phase 8 operator test binary runs, **Then** the live-order test is ignored.
2. **Given** exact approval variables are absent, **When** the ignored operator test is explicitly selected, **Then** it fails before building the LiveNode.
3. **Given** a live order remains open, **When** cleanup evidence is required, **Then** evidence must come from configured strategy/NT behavior, not direct exec-engine commands.

---

### Edge Cases

- Phase 7 local branch exists but Phase 7 is not merged to `main`: Phase 8 blocks and records stale dependency.
- PR #320 plan contains valid requirements but is stacked on closed PR #319: use as forensic input only, not as base or accepted scope.
- Real no-submit report exists but is not accepted by the live canary gate: Phase 8 blocks.
- Chainlink feed id cannot be source-verified as the exact ETH/USD production feed for the configured environment: Phase 8 live action blocks.
- Polymarket fee/rebate assumptions are not contract-backed for the current NT pin: Phase 8 live action blocks.
- NT adapter submit/accept/reject/fill/cancel/reconciliation evidence is unavailable: Phase 8 live action blocks.
- Runtime capture cannot be wired or verified: Phase 8 blocks before live order approval.

## Requirements

### Functional Requirements

- **FR-001**: Phase 8 MUST start from current `main` / `origin/main`; stale PR #318, #319, and #320 content is reference-only forensic input.
- **FR-002**: Phase 8 MUST NOT place any live order unless the user explicitly approves an exact command and exact head SHA in the current thread.
- **FR-003**: Phase 8 MUST block live order approval until Phase 7 authenticated no-submit readiness is present on `main` or an explicitly approved base and a real redacted no-submit report is accepted by the live canary gate.
- **FR-004**: Phase 8 MUST consume the existing Phase 6 `BoltV3SubmitAdmissionState` and MUST NOT add a second submit admission path.
- **FR-005**: Phase 8 MUST use the production bolt-v3 LiveNode path through `build_bolt_v3_live_node` and `run_bolt_v3_live_node` within a `tokio::task::LocalSet` context; direct `LiveNode::run` in Phase 8 harness code is forbidden.
- **FR-006**: Phase 8 MUST produce redacted dry/no-submit canary evidence before any live-order command can be considered.
- **FR-007**: Canary evidence MUST join decision intent, live canary gate result, submit admission result, NT runtime capture identity, and observed NT order lifecycle evidence where applicable.
- **FR-008**: Phase 8 MUST rely on NT for order submit, accept, reject, fill, cancel, restart reconciliation, cache state, and adapter behavior.
- **FR-009**: Phase 8 MUST NOT implement Bolt-owned order lifecycle, Bolt-owned reconciliation, mock venue proof, adapter forks, cache forks, or alternate secret sources.
- **FR-010**: Phase 8 MUST fail closed if strategy-input safety audit is blocked or incomplete.
- **FR-011**: Strategy-input safety audit MUST cover Chainlink feed id correctness and environment, Data Streams semantics, Binance/Bybit/OKX/Kraken/Deribit/Hyperliquid reference venue semantics, weighting/staleness/disable rules, realized volatility including fail-closed handling for non-positive values, `pricing_kurtosis`, theta decay, fee/rebate assumptions, market selection, option pricing including fail-closed handling for non-positive time to expiry, edge threshold economics, adverse selection, liquidity, spread, and book impact.
- **FR-012**: Phase 8 MUST keep SSM as the only credential source and MUST NOT add env-var credential fallback. Operator env vars may identify non-secret paths, hashes, approval ids, and exact commands only.
- **FR-013**: Phase 8 MUST preserve pure Rust runtime behavior and MUST NOT introduce a Python runtime layer.
- **FR-014**: Phase 8 MUST include behavior tests before implementation for each canary slice.
- **FR-015**: Phase 8 MUST stop at local implementation readiness if external review, CI, or exact-head push approval is unavailable.

### Key Entities

- **Phase8CanaryPreflight**: Redacted status object that records dependency checks, exact head, config checksum, readiness report status, strategy-input audit status, and live-capital block reason.
- **Phase8CanaryEvidence**: Redacted artifact that records dry or live canary outcome, cap values, config identity hashes, approval identity, decision evidence reference, submit admission result, and NT lifecycle evidence references.
- **StrategyInputSafetyAudit**: Evidence-backed approve/block record for `eth_chainlink_taker` strategy inputs and market assumptions.
- **OperatorApprovalEnvelope**: Non-secret operator-supplied values required for the ignored live harness: exact head SHA, approval id, root TOML path, root TOML checksum, SSM manifest hash, and evidence output path.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Default local tests run zero live orders and report the live operator harness ignored.
- **SC-002**: Preflight tests prove missing Phase 7 report, rejected live canary gate, missing strategy audit approval, and approval mismatch all block before `run_bolt_v3_live_node`.
- **SC-003**: Source fences prove Phase 8 harness code contains no direct submit, direct cancel, direct reconciliation report synthesis, or direct `LiveNode::run`.
- **SC-004**: Dry/no-submit evidence fixture serializes without raw secrets and contains the required join keys.
- **SC-005**: Strategy-input audit produces an explicit approve/block recommendation before any live action.
- **SC-006**: External reviewers approve spec/plan/tasks before Phase 8 implementation starts.

## Assumptions

- Phase 7 local work exists but is not yet merged to main as of `d6f55774c32b71a242dcf78b8292a7f9e537afab`; Phase 8 must treat it as a dependency, not accepted main scope.
- `config/live.local.toml` is gitignored and absent from the fresh Phase 8 worktree; any real operator config must be approved and inspected through redacted structural checks only.
- Current main includes Phase 6 submit admission and live canary gate, but not Phase 7 no-submit readiness producer.
- Actual live order execution is outside this spec until explicit approval names the exact head and command.
