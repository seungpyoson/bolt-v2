# Feature Specification: Production Live Readiness

**Feature Branch**: `issue-369-production-readiness`
**Created**: 2026-05-18
**Status**: Draft
**Input**: User description: "Issue #369: define production-grade live trading readiness beyond Issue #360 tiny-canary readiness."

## User Scenarios & Testing

### User Story 1 - Prevent Overstated Readiness Claims (Priority: P1)

As the operator, I can see exactly which readiness level is supported by current evidence so one tiny canary cannot be presented as production-grade live trading.

**Why this priority**: This blocks the main Issue #369 failure mode: conflating one controlled canary with repeated production operation.

**Independent Test**: A doc/test gate proves the production-readiness contract defines tiny-canary, staged-live, and production-live levels and blocks broader claims without evidence.

**Acceptance Scenarios**:

1. **Given** a PR or issue claims live readiness, **When** evidence supports only one controlled canary, **Then** the allowed claim is tiny-canary readiness only.
2. **Given** a PR or issue claims production readiness, **When** staged-live and production-live evidence is absent, **Then** the claim is blocked or explicitly waived by the operator.

---

### User Story 2 - Gate Repeated Live Operation (Priority: P2)

As the operator, I can identify the exact evidence required before repeated live runs are proposed.

**Why this priority**: Repeated operation introduces risks not covered by a one-canary proof, including restart, replay, monitoring, and hygiene failures.

**Independent Test**: The readiness contract and SpecKit artifacts link runbooks and required tests/tooling for repeated operation, abort, restart recovery, and post-run hygiene.

**Acceptance Scenarios**:

1. **Given** a staged-live promotion request, **When** order lifecycle, restart reconciliation, single-runner, approval replay, monitoring, and deploy provenance evidence is missing, **Then** staged-live readiness remains blocked.
2. **Given** repeated-live operation is proposed, **When** runbook links are absent, **Then** the promotion fails requirements validation.

---

### User Story 3 - Tie Production Claims To Deploy Provenance (Priority: P3)

As the operator, I can verify production-live claims against the reviewed commit, built binary, host, root TOML, SSM manifest, and approval artifact.

**Why this priority**: Production trading requires deploy/run trust beyond local code readiness.

**Independent Test**: The production-readiness contract requires deploy provenance and monitoring proof before production-live claims.

**Acceptance Scenarios**:

1. **Given** a production-live claim, **When** reviewed commit, binary, host, root TOML, SSM manifest, approval artifact, NT pin, or CI evidence is missing, **Then** the claim is blocked.
2. **Given** any evidence package contains raw secrets, raw approval ids, private keys, or account balances, **When** it is reviewed, **Then** it is invalid for promotion.

### Edge Cases

- A reviewer sees a green local test suite but no real venue evidence: the claim remains bounded to local readiness.
- A tiny canary completes but restart reconciliation or post-run hygiene evidence is missing: staged-live readiness remains blocked.
- A deploy host runs a different binary or root TOML than reviewed: production-live readiness remains blocked.
- A required blocker in status-map rows 34-48 remains open: production-live readiness remains blocked unless explicitly waived by the operator.

## Requirements

### Functional Requirements

- **FR-001**: The repo MUST define readiness levels after Issue #360: tiny-canary ready, staged-live ready, and production-live ready.
- **FR-002**: The repo MUST record exact evidence required to promote between readiness levels.
- **FR-003**: The repo MUST add or link runbook requirements for repeated-live operation, abort, restart recovery, and post-run hygiene.
- **FR-004**: The repo MUST add or link tests/tooling requirements for order lifecycle, restart reconciliation, single-runner protection, and approval replay resistance beyond one canary.
- **FR-005**: The repo MUST add or link monitoring and alerting requirements for staged-live and production-live operation.
- **FR-006**: The repo MUST add or link deploy provenance requirements tying reviewed commit, built binary, host, root TOML, SSM manifest, approval artifact, NT pin, and CI run.
- **FR-007**: The repo MUST block production-grade live trading language unless the production-readiness checklist is satisfied or explicitly waived by the operator.
- **FR-008**: The repo MUST keep Issue #369 separate from live submit approval; this feature must not run live capital.
- **FR-009**: The repo MUST reject evidence packages that expose raw secrets, raw approval ids, private keys, or account balances.
- **FR-010**: The repo MUST keep no-submit readiness, tiny-canary readiness, staged-live readiness, and production-live readiness as separate claims.

### Key Entities

- **ReadinessLevel**: Named claim level with allowed claim language and required evidence.
- **PromotionEvidencePackage**: Redacted collection of exact-head, config, approval, runtime, monitoring, and run-result evidence.
- **OperatorRunbook**: Procedure required before repeated live operation, abort handling, restart recovery, and post-run hygiene.
- **ProductionClaimBlocker**: Missing evidence, stale proof, unresolved status-map blocker, or unwaived reviewer finding that blocks broader live claims.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Issue #369 acceptance criteria are represented by concrete docs, SpecKit artifacts, and a cargo test gate.
- **SC-002**: A reviewer can distinguish tiny-canary, staged-live, and production-live readiness from the docs without relying on chat history.
- **SC-003**: A production-live claim can be rejected by checking a finite evidence checklist instead of subjective judgment.
- **SC-004**: Local verification proves the readiness contract and SpecKit artifacts exist and name all required Issue #369 gates.

## Assumptions

- Issue #360 remains scoped to one controlled tiny-capital canary path.
- Issue #369 is a readiness-definition slice, not approval to submit live orders.
- Missing staged-live and production-live tooling may be listed as blockers when the acceptance criterion asks to add or link requirements rather than implement every blocker.
- Existing status-map rows 34-48 remain source-backed blocker references until implementation slices close them.
