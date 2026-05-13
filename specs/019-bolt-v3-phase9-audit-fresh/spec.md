# Feature Specification: Bolt-v3 Phase 9 Comprehensive Audit

**Feature Branch**: `019-bolt-v3-phase9-audit-fresh`
**Created**: 2026-05-14
**Status**: Draft
**Input**: User description: "Audit bolt-v3 architecture, hardcoded runtime values, dual paths, debt, NT boundary, SSM-only secrets, pure Rust runtime, stale specs/tasks, test quality, external review disposition, strategy assumptions, and live ops readiness after Phase 7/8 readiness. Do not merge. Do not perform live capital or soak without explicit approval."

## User Scenarios & Testing

### User Story 1 - Evidence-Backed Audit Report (Priority: P1)

The operator can read one current-main audit report that separates accepted evidence, gaps, and blockers across architecture, config, secrets, NT ownership, strategy input assumptions, tests, stale artifacts, and live operations.

**Why this priority**: Phase 9 exists to decide whether bolt-v3 is ready for no-submit only, ready for tiny live order approval, or blocked. That decision must be source-backed.

**Independent Test**: Review `audit-report.md` and verify every material finding has at least one file/line citation, command output, PR metadata, test output, or reviewer job record.

**Acceptance Scenarios**:

1. **Given** current main at `d6f55774c32b71a242dcf78b8292a7f9e537afab`, **When** the Phase 9 audit report is reviewed, **Then** it states that final Phase 9 readiness is blocked until fresh Phase 7 and Phase 8 branches are accepted or explicitly waived.
2. **Given** a readiness claim, **When** the claim is checked, **Then** it maps to concrete evidence and does not rely on stale branch code.
3. **Given** a live-capital claim, **When** the report is checked, **Then** it blocks live action unless the exact head, exact command, and user approval are present.

---

### User Story 2 - Cleanup Gate (Priority: P2)

The operator can see exactly which cleanup is allowed, which cleanup is blocked, and which behavior tests must exist before any code cleanup starts.

**Why this priority**: Phase 9 includes AI slop cleanup only after behavior tests lock the target behavior; cleanup must not become unreviewed refactor drift.

**Independent Test**: Review `ai-slop-cleanup-report.md` and `tasks.md` and confirm no cleanup task edits runtime code before a targeted behavior test, reviewed plan, and user approval.

**Acceptance Scenarios**:

1. **Given** stale docs or weak verifier coverage, **When** cleanup is proposed, **Then** the proposal names the exact file scope and behavior test that must pass before the edit.
2. **Given** an untested code cleanup candidate, **When** Phase 9 is executed, **Then** the task list blocks that cleanup and records the missing evidence.

---

### User Story 3 - Review-Ready Plan (Priority: P3)

The operator can send the Phase 9 spec, checklist, plan, tasks, audit report, and cleanup report to external reviewers and triage findings before any implementation branch work proceeds.

**Why this priority**: User policy requires external approval before implementation and at least Claude, DeepSeek, and GLM for this session.

**Independent Test**: After the branch is clean and pushed, run external plan reviews against exact head and record all findings in a disposition file before code implementation starts.

**Acceptance Scenarios**:

1. **Given** a dirty, unpushed, or stale branch, **When** external review is requested, **Then** the workflow stops before source transmission.
2. **Given** a direct API reviewer, **When** source is sent, **Then** approval-token flow records `source_content_transmission: not_sent` before user-approved transmission proceeds.
3. **Given** a blocking reviewer finding, **When** triage runs, **Then** the finding is accepted and fixed or disproved with evidence before implementation.

## Edge Cases

- Phase 7 or Phase 8 local branches exist but are not pushed, reviewed, or merged into main.
- A stale PR branch contains useful text but conflicts with current main.
- A fixture or documentation literal looks like a runtime hardcoded value.
- A Python verification script exists even though the runtime must remain a pure Rust binary.
- A strategy input is configurable but its economic or feed-source assumption is not production-approved.
- Live ops evidence exists as incident history, but not as an approved current runbook, alert, rollback, and incident-response package.

## Requirements

### Functional Requirements

- **FR-001**: The audit MUST start from current `main` and MUST NOT treat PR #318, PR #319, or PR #320 as accepted implementation scope.
- **FR-002**: The audit MUST classify current Phase 7, Phase 8, and Phase 9 readiness separately.
- **FR-003**: The audit MUST cover hardcoded runtime values, dual paths, debt markers, brittle architecture, AI slop, NT boundary violations, SSM-only secret source, pure Rust runtime, runtime config grouping, stale docs/specs/tasks, source fences, test quality, external review disposition, production readiness gaps, strategy math/feed assumptions, and live ops readiness.
- **FR-004**: The audit MUST distinguish runtime code from tests, fixtures, docs, and verification scripts before labeling a finding as a production violation.
- **FR-005**: The audit MUST fail closed on any unresolved live-capital, secret-exposure, Chainlink/feed-source, strategy-math, NT-boundary, or external-review blocker.
- **FR-006**: The audit MUST state that no live order, no soak, and no real-capital command can run without explicit user approval for exact head/SHA and command.
- **FR-007**: Cleanup MUST be bounded, behavior-test protected, externally reviewed when material, and user-approved before implementation.
- **FR-008**: External review of Phase 9 artifacts MUST include Claude, DeepSeek, and GLM at minimum before implementation.
- **FR-009**: Direct API reviewer source transmission MUST use approval-token records and MUST not expose raw secrets.
- **FR-010**: The final recommendation MUST be one of: ready for no-submit only, ready for tiny live order approval, blocked with exact blockers, or stop.

### Key Entities

- **AuditFinding**: A severity-ranked finding with category, evidence, decision impact, and recommended next action.
- **EvidenceCitation**: File/line, command output, PR metadata, reviewer job record, or test output supporting one claim.
- **CleanupCandidate**: A bounded code or doc cleanup item with required behavior tests and stop conditions.
- **ExternalReviewDisposition**: Reviewer job identity, approval status, findings, and accept/disprove/defer decision.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Audit report covers every FR-003 category with evidence or a named evidence gap.
- **SC-002**: No Phase 9 implementation task can begin until spec, checklist, plan, tasks, and external-review disposition are present on a clean pushed branch.
- **SC-003**: All live-capital paths remain blocked unless explicit user approval includes exact head/SHA and command.
- **SC-004**: Debt-marker scan over Phase 9 artifacts returns no unresolved template markers.
- **SC-005**: no-mistakes runtime proof is recorded before external review and before any PR readiness claim.

## Assumptions

- Phase 9 in this branch is an audit and planning slice from current main, not a final post-Phase7/8 readiness certification.
- Fresh Phase 7 and Phase 8 local branches may inform residual scope, but until pushed, reviewed, accepted, and merged, main remains the only accepted runtime source.
- The exact active local operator config is intentionally untracked; absence of `config/live.local.toml` in this checkout is not a secret failure by itself, but blocks source-backed live readiness.
