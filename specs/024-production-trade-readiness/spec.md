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

As operator, I need T128/T130/T131/T122 and T116/T046 to run only after real artifacts exist, exact-head verification is green, and external reviewers approve.

**Independent Test**: `operator-artifacts verify-final` passes on the final root TOML and packet; then final-packet EC2/EIP no-submit passes; then tiny-capital canary passes.

## Requirements

- **FR-001**: Use `specs/024-production-trade-readiness/` as the explicit PR #480 trade-readiness task packet; do not implement order-intent work from `specs/023-nt-order-intent-layer/`.
- **FR-002**: Keep #466 verifier decomposition outside this feature unless the operator explicitly changes scope.
- **FR-003**: Use one readiness PR; do not create PR-per-slice churn.
- **FR-004**: Do not implement production code until the task list receives task-list approvals from Claude, Gemini, DeepSeek, GLM, Kimi, and Grok, or until the operator explicitly waives an unavailable reviewer with exact failure evidence.
- **FR-005**: Use TDD for each code slice: RED evidence first, minimal implementation, then verification.
- **FR-006**: Runtime values must remain config-owned through TOML or operator evidence files.
- **FR-007**: Do not claim production trade readiness from unit tests, fixture artifacts, static manifests, or historical no-submit reports.
- **FR-008**: Run final exact-head CI and external review before approved live/no-submit/canary operations.

## Success Criteria

- **SC-001**: T124/T125 bind real current-head runtime decision evidence into the final packet.
- **SC-002**: T126/T127 source-owned collectors exist for all required pre-run and abort fields.
- **SC-003**: T128 final packet is blocker-free and verified against root TOML operator evidence.
- **SC-004**: T130 exact-head local verification, GitHub CI, and external review pass.
- **SC-005**: T131/T122 final-packet EC2/EIP no-submit passes.
- **SC-006**: T116/T046 tiny-capital canary passes after no-submit.
