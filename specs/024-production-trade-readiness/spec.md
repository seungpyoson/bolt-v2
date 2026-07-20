# Feature Specification: Production Trade Readiness

**Feature Branch**: `goal/024-production-trade-readiness`
**Created**: 2026-05-25
**Status**: Evidence-first task baseline

## Scope

Finish production-grade trade readiness for the existing bolt-v3 live path. This feature is not the order-intent layer and is not #466 verifier decomposition work.

## Evidence Inputs

This task list was rebuilt from current hard evidence, not from the stale active Spec Kit pointer:

- Current git and PR state for PR #480, historical PR #478, and PR #479.
- GitHub issues #369, #385, #409, and #360.
- `specs/001-thin-live-canary-path/tasks.md`.
- `docs/bolt-v3/2026-05-23-pr388-t124-t128-root-problem-memos.md`.
- Current readiness source inspection of `src/bolt_v3_operator_artifacts.rs` and `tests/bolt_v3_operator_artifacts.rs`.
- Fetched branch `origin/t038-operator-config-snapshot` at `53c43608e74d7d8293c8830f57ed180d94bb7c5a`.
- External T036H gate-architecture review recorded in `specs/024-production-trade-readiness/external-gate-architecture-review.md`.
- Concrete T036H end-to-end gate contract recorded in `specs/024-production-trade-readiness/gate-dataflow-contract.md`.

The command-level evidence is recorded in `specs/024-production-trade-readiness/evidence.md`.

## User Stories

### User Story 1 - Scope And Baseline Are Unambiguous (Priority: P1)

As operator, I need one readiness PR and one current readiness task list so implementation does not drift into order-intent or #466 work.

**Independent Test**: PR #480 references this feature on `goal/024-production-trade-readiness`, PR #479 remains excluded, non-readiness files are removed from the PR, and the readiness task list has six-reviewer task-list approvals before implementation resumes.

### User Story 2 - Real Decision Evidence Is Bound (Priority: P1)

As operator, I need T124/T125 artifacts to come from real current-head runtime decision evidence, not static generation or fixtures.

**Independent Test**: final packet generation rejects missing real market-selection and strategy-input evidence, then passes only when paths and hashes are bound through `[live_canary.operator_evidence]`.

### User Story 3 - Pre-Run State Is Source-Proven (Priority: P1)

As operator, I need T126 to prove account, open-order, position, funding, margin, egress, and CLOB V2 readiness from source-owned collectors.

**Independent Test**: pre-run proof generation fails when any source-owned collector output is missing and passes only with bounded, non-secret evidence for every required field.

### User Story 4 - Abort Paths Are Source-Proven (Priority: P1)

As operator, I need T127 to prove every required abort path before a tiny-capital canary can run.

**Independent Test**: abort-plan proof generation fails without source-owned cancel-if-open, accepted/pending, partial-fill, network-partition, and panic/service-policy evidence.

### User Story 5 - Final Packet And Live Readiness Chain Complete (Priority: P1)

As operator, I need T128/T130/T131/T122 and T116/T046 to run only after real artifacts exist and exact-head verification is green.

**Independent Test**: `operator-artifacts verify-final` passes on the final root TOML and packet; then final-packet EC2/EIP no-submit passes; then tiny-capital canary passes.

## Requirements

- **FR-001**: Use `specs/024-production-trade-readiness/` as the explicit PR #480 trade-readiness task packet; do not implement order-intent work from `specs/023-nt-order-intent-layer/`.
- **FR-002**: Keep #466 verifier decomposition outside this feature unless the operator explicitly changes scope.
- **FR-003**: Use one readiness PR; do not create PR-per-slice churn.
- **FR-004**: Do not implement production code until the operator approves the task-list scope.
- **FR-005**: Use TDD for each code slice: RED evidence first, minimal implementation, then verification.
- **FR-006**: Runtime values must remain config-owned through TOML or operator evidence files.
- **FR-007**: Do not claim production trade readiness from unit tests, fixture artifacts, static manifests, or historical no-submit reports.
- **FR-008**: Run final exact-head CI before approved live/no-submit/canary operations.
- **FR-009**: Resolution/reference readiness gates must be market, venue, account, value-kind, and provider agnostic: Chainlink, Pyth, exchange-index, HIP-4/venue-native, Deribit/index, outcome-oracle, sports, politics, entertainment, and no-resolution markets are selected by config and selected-market metadata, not by hardcoded archetype assumptions.
- **FR-010**: Strategy archetypes may declare required gate roles/classes/value-kinds only; provider-specific feed ids, schema versions, decimal scales, freshness windows, endpoints, venue metadata scopes, and credentials belong to TOML-owned gate provider/subscription config and provider validators.
- **FR-011**: Dynamic market rotation must fail closed unless the selected market requirement, configured target subscription, provider capability, and evidence all match for the same selected market identity and gate role.
- **FR-012**: Entry readiness must produce the validated gate/evidence session consumed by runtime strategy logic; strategy logic must not bypass readiness through a second unchecked provider path.
- **FR-013**: The gate TOML schema must use root `[gate_providers.<id>]` blocks and per-target `[target.gate_subscriptions.<role>]` blocks; provider-specific fields are valid only under the matching gate provider block.
- **FR-014**: Example strategy TOML and test fixtures must be migrated with the gate schema so shipped configs do not retain provider-specific runtime fields under archetype parameters.
- **FR-015**: Decision evidence, tiny-canary evidence, CLI artifact commands, strategy registration, runtime strategy logic, and source replay must consume readiness-created gate sessions or normalized evidence identities; a provider-specific source string alone must not satisfy readiness.
- **FR-016**: T036H implementation must follow the boundary contract in `specs/024-production-trade-readiness/gate-dataflow-contract.md`; any deviation requires a recorded disposition before code changes.
- **FR-017**: Live-canary and final-packet readiness must bind the readiness gate session path and sha256 for every strategy instance with required gate roles; absence, hash mismatch, selected-market mismatch, stale evidence, or role mismatch must fail closed.
- **FR-018**: Every data-client adapter added or enabled by this PR must have a recorded production-readiness matrix before it is described as production-usable. Metadata-only REST smoke evidence is insufficient; the matrix must prove config-owned LiveNode wiring, data freshness/subscription behavior or a fail-closed unsupported-path disposition, reconnect/rate-limit/error behavior, credential/no-execution boundaries for data-only clients, and no venue or market hardcodes.
- **FR-019**: Strategy files may produce order intent and strategy-local signal state only. Execution admissibility, venue rules, fillability, rounding, minimum order size, fee-adjusted sizing, and submit gating must live in shared execution/admission modules built on NT APIs. Any change under `src/strategies/*` that handles submit mechanics requires an explicit recorded approval that the behavior is strategy-local signal logic.

## Success Criteria

- **SC-001**: T124/T125 bind real current-head runtime decision evidence into the final packet.
- **SC-002**: T126/T127 source-owned collectors exist for all required pre-run and abort fields.
- **SC-003**: T128 final packet is blocker-free and verified against root TOML operator evidence.
- **SC-004**: T130 exact-head local verification and GitHub CI pass.
- **SC-005**: T131/T122 final-packet EC2/EIP no-submit passes.
- **SC-006**: T116/T046 tiny-capital canary passes after no-submit.
- **SC-007**: T036H RED/GREEN coverage proves no provider or venue is globally required and proves mismatched, stale, wrong-role, or wrong-value-kind resolution/reference evidence cannot satisfy a rotated selected market.
- **SC-008**: T036H RED/GREEN coverage proves the runtime strategy receives only a readiness-created gate session or normalized evidence object and cannot open a second unchecked provider path.
- **SC-009**: T036H RED/GREEN coverage proves old `price_to_beat_source` string comparisons cannot satisfy decision evidence, tiny-canary evidence, or CLI final-packet readiness without the matching readiness session/evidence identity.
- **SC-010**: T036H RED/GREEN coverage proves live-canary and final-packet verification reject required-role strategy instances unless the operator packet binds the matching gate session path and sha256.
- **SC-011**: The data-client production-readiness matrix passes for every PR-enabled data client before final readiness closeout or any production-usable multi-venue data-client claim.
