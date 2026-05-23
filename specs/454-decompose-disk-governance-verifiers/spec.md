# Feature Specification: Decompose Disk-Governance Verifiers

**Feature Branch**: `codex/454-decompose-disk-governance-verifiers`  
**Created**: 2026-05-23  
**Status**: Draft  
**Input**: GitHub issue #454, "Decompose disk-governance verifier scripts after #436"

## User Scenarios & Testing

### User Story 1 - Preserve Accepted Command Classification (Priority: P1)

As a maintainer, I need representative current command-understanding behavior captured before any decomposition so a refactor cannot silently change accepted verifier semantics.

**Why this priority**: #454 exists because runtime enforcement and static workflow hygiene now duplicate command understanding. Characterization is the guardrail that makes later movement reviewable.

**Independent Test**: A reviewer can run focused characterization/parity tests and see that representative shell, cargo, wrapper, Python inline, renamed-tool, and target-routing inputs preserve each current surface classification before and after extraction. Only helpers proven equivalent by pre-extraction comparison may move behind the shared module.

**Acceptance Scenarios**:

1. **Given** the current verifier behavior on main, **When** representative command samples are classified by the characterization suite, **Then** the expected current classifications are documented and enforced.
2. **Given** a future extraction changes command parsing behavior, **When** the characterization suite runs, **Then** the behavior change fails locally unless the PR explicitly documents and approves the scope change.

---

### User Story 2 - Share Parser Logic Without New Semantics (Priority: P2)

As a maintainer, I need parser/scanner helpers that are proven equivalent isolated behind one shared path so runtime cleanup enforcement and static workflow hygiene do not drift independently.

**Why this priority**: Related helper families now exist in multiple oversized files, including command tokenization, shell command substitution parsing, Python inline command parsing, wrapper handling, renamed cargo/rustc detection, and target-routing checks. Some same-named helpers are semantically divergent, so the extraction boundary must be proven rather than assumed.

**Independent Test**: After a mechanical extraction of proven-equivalent helpers, existing tests plus parity tests pass without adding wrapper families, shell semantics, regex cases, or command-prediction behavior. Divergent helpers remain surface-specific and are documented as such.

**Acceptance Scenarios**:

1. **Given** candidate parser helpers in the verifier surfaces, **When** pre-extraction comparison proves a helper family equivalent, **Then** both verifier surfaces use the shared path for that extracted behavior.
2. **Given** issue #454 forbids new semantics, **When** the PR is reviewed, **Then** any observed behavior changes are either absent or explicitly backed by characterization evidence and operator approval.

---

### User Story 3 - Keep Remaining Verifier Surfaces Reviewable (Priority: P3)

As a maintainer, I need any remaining oversized verifier/test surfaces split only when the split is mechanical and independently reviewable.

**Why this priority**: The issue goal is maintenance risk reduction, not a broad verifier redesign. A small, reviewable PR is more valuable than a cosmetic split that hides behavior changes.

**Independent Test**: Reviewers can inspect the evidence map and task checklist to see which surfaces were reduced, which were intentionally left alone, and why remaining risk is bounded.

**Acceptance Scenarios**:

1. **Given** a candidate file split, **When** the split is reviewed, **Then** it moves cohesive existing tests or helpers without changing behavior.
2. **Given** a candidate split would require redesign or new semantics, **When** scope is assessed, **Then** the split is deferred and recorded as remaining risk.

### Edge Cases

- Existing inputs that are intentionally conservative or fail-closed must remain conservative unless the operator explicitly approves a semantic change.
- Raw cargo/storage detection fixtures must not be weakened by moving parser code into a shared module.
- Characterization fixtures must cover both runtime-owned cleanup enforcement and static workflow/no-mistakes hygiene paths.
- Same-named helpers with divergent behavior must remain surface-specific unless the PR explicitly documents, tests, and receives operator approval for a semantic change.
- Any external review slot exceeding 15 minutes is recorded as skipped, not approved.
- no-mistakes is not part of this issue unless the operator explicitly requests it.

## Requirements

### Functional Requirements

- **FR-001**: The PR MUST include an evidence map that separates current behavior, latent risk, and future enablement requirements for #454.
- **FR-002**: The PR MUST add characterization or parity tests before moving shared command/cargo parsing behavior.
- **FR-003**: The characterization surface MUST cover representative shell command boundaries, command substitutions, Python inline commands, current wrapper handling, cargo subcommand scanning, renamed cargo/rustc detection, and target-routing override detection. Coverage may be surface-specific where current behavior differs.
- **FR-004**: The implementation MUST reduce or isolate every pre-extraction-proven equivalent parser/scanner helper family behind one shared path. Similar-name or analogous helpers that are divergent MUST remain surface-specific unless a semantic change is separately documented, tested, and operator-approved.
- **FR-005**: The implementation MUST preserve accepted behavior for representative current inputs unless a behavior change is separately documented, tested, and operator-approved.
- **FR-006**: The implementation MUST NOT add new wrapper families, shell semantics, regex edge cases, command-prediction behavior, or policy surface.
- **FR-007**: The PR MUST remain scoped to #454 and MUST NOT include #375 follow-up work or unrelated verifier redesign.
- **FR-008**: The PR body MUST state scope, non-goals, verification, remaining risk, external review status, and that merge still requires operator approval.
- **FR-009**: Exact-head GitHub CI and external review MUST be green before the PR is presented as review-ready.
- **FR-010**: External review readiness MUST include at least one non-skipped adversarial review approval and no unresolved blocking findings. Skipped, timed-out, or stale-head slots are not approvals.

### Key Entities

- **Characterization Case**: A representative input command or workflow snippet plus the current expected classification result.
- **Shared Command Understanding Path**: The single reusable parser/scanner path that extracted verifier logic uses after decomposition.
- **Verifier Surface**: A runtime or static governance script and its test surface affected by this issue.
- **Evidence Map**: A planning and PR artifact that maps current behavior, duplication risk, verification coverage, and intentionally deferred work.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Existing issue-named suites pass: `python3 scripts/test_rust_verification_cache_retention.py` and `python3 scripts/test_verify_ci_workflow_hygiene.py`.
- **SC-002**: The touched Python files compile with `python3 -m py_compile`.
- **SC-003**: `git diff --check` reports no whitespace errors.
- **SC-004**: Every helper family proven equivalent by the pre-extraction comparison is moved or isolated behind a shared path used by both relevant verifier surfaces, and every divergent candidate is recorded with a deferral or operator-approved change rationale.
- **SC-005**: Characterization/parity tests fail on representative drift and pass after the mechanical extraction.
- **SC-006**: Exact-head GitHub CI is green before review-ready handoff.
- **SC-007**: External exact-head review reports no blocking findings, with skipped slots recorded if a reviewer exceeds the 15-minute limit.

## Assumptions

- #375 is complete and closed before this issue starts.
- The accepted behavior from PR #436 and current `main` is the baseline unless the operator explicitly approves a change.
- The first PR should prefer a narrow shared parser extraction over broad file reorganization.
- no-mistakes is intentionally out of scope unless the operator explicitly requests it.
