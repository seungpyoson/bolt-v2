# Feature Specification: Phase 6 Submit Admission Recovery

**Feature Branch**: `[002-phase6-submit-admission-recovery]`  
**Created**: 2026-05-13  
**Status**: Ready for Planning  
**Input**: User description: "Recover Phase 6 submit admission from the stale PR chain without reintroducing stale Phase 3-5 work, using prior PR #317 only as reference material and requiring evidence, anti-slop cleanup, recovery-strategy review before implementation, and code review only after exact-head CI is green."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Decide What To Salvage (Priority: P1)

As the maintainer, I need a concise recovery decision that separates valid Phase 6 submit-admission work from stale branch drift so I can proceed without trusting obsolete PR topology.

**Why this priority**: This prevents duplicate work, accidental rollback of merged Phase 3-5 architecture, and review waste.

**Independent Test**: Can be fully tested by reviewing the recovery memo and confirming every stale PR item is classified as keep, rewrite, or reject with evidence from current `main`.

**Acceptance Scenarios**:

1. **Given** a stale Phase 6 PR whose base predates current `main`, **When** the recovery decision is prepared, **Then** the PR is marked reference-only and not treated as mergeable.
2. **Given** a Phase 6 idea from the stale PR, **When** it is classified, **Then** the classification explains whether to keep, rewrite, or reject it and why.
3. **Given** current `main` already contains Phase 3-5 architecture, **When** the Phase 6 recovery scope is defined, **Then** no stale Phase 3-5 code is allowed into the new work.

---

### User Story 2 - Validate The Recovery Strategy With Second Opinions (Priority: P2)

As the maintainer, I need recovery-strategy reviewers to evaluate the salvage plan before more code is written so architectural drift is caught before implementation cost increases.

**Why this priority**: The main risk is decision quality, not just local code correctness.

**Independent Test**: Can be fully tested by sending recovery-strategy reviewers a prompt containing exact current and stale PR SHAs, the proposed keep/rewrite/reject map, and explicit questions about architecture, scope, and drift.

**Acceptance Scenarios**:

1. **Given** the recovery strategy is drafted, **When** a recovery-strategy review prompt is generated, **Then** it asks reviewers to challenge whether the recovery path is correct before coding.
2. **Given** recovery-strategy review feedback arrives, **When** findings are assessed, **Then** every substantive issue is either fixed in the recovery plan or explicitly rejected with evidence.

---

### User Story 3 - Produce One Fresh Phase 6 PR (Priority: P3)

As the maintainer, I need one fresh Phase 6 PR from current `main` that implements submit admission without carrying stale branch changes.

**Why this priority**: A narrow fresh PR reduces CI cost, review cost, and risk of undoing merged work.

**Independent Test**: Can be fully tested by comparing the fresh PR against current `main` and confirming it touches only the approved Phase 6 surfaces.

**Acceptance Scenarios**:

1. **Given** the fresh Phase 6 branch is created from current `main`, **When** the diff is reviewed, **Then** it contains no stale Phase 3-5 churn from old PRs.
2. **Given** the submit-admission behavior is implemented, **When** targeted tests run, **Then** evidence is recorded before admission and admission occurs before order submission.
3. **Given** CI is green for the exact PR head, **When** external code review is requested, **Then** reviewers evaluate the current diff rather than stale PR history.

### Edge Cases

- The stale PR contains a valid Phase 6 idea embedded in obsolete surrounding code.
- A reviewer recommends cherry-picking the stale PR directly.
- The fresh Phase 6 diff accidentally changes Phase 3-5 behavior.
- The recovery prompt lacks exact SHAs or current-main evidence.
- A future stacked PR depends on stale Phase 6 branch history instead of the fresh branch.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The recovery process MUST treat current `main` as the only source of truth after merged work.
- **FR-002**: The recovery process MUST classify stale PR #317 material as keep, rewrite, or reject in `specs/002-phase6-submit-admission-recovery/recovery-review.md` before implementation begins.
- **FR-003**: The recovery process MUST forbid direct merge, direct rebase, or broad cherry-pick of stale PR #317.
- **FR-004**: The recovery process MUST preserve already-merged Phase 3-5 architecture and reject changes that reintroduce older registration, evidence, runtime-capture, or helper-path designs.
- **FR-005**: The recovery process MUST define the fresh Phase 6 scope before code changes begin.
- **FR-006**: The fresh Phase 6 scope MUST be limited to submit-admission behavior and the minimum required wiring, tests, and documentation.
- **FR-007**: The recovery process MUST require recovery-strategy review before fresh Phase 6 implementation begins.
- **FR-008**: The recovery-strategy review prompt MUST include exact current-main SHA, stale PR head SHA, stale PR base context, and the keep/rewrite/reject map.
- **FR-009**: Every recovery-strategy review finding MUST be resolved, disproved with evidence, or explicitly deferred before implementation proceeds.
- **FR-010**: The fresh Phase 6 PR MUST be created from current `main`, not from the stale PR chain.
- **FR-011**: The fresh Phase 6 PR MUST not include unrelated cleanup, stale fixture churn, broad docs churn, or Phase 7-8 behavior.
- **FR-012**: The fresh Phase 6 PR MUST demonstrate that decision evidence remains mandatory before submit admission, and submit admission remains mandatory before order submission.
- **FR-013**: The recovery process MUST include an anti-slop cleanup pass that checks for duplication, dead code, needless abstraction, boundary violations, and missing regression coverage.
- **FR-014**: The recovery process MUST not request external code review until the exact fresh PR head has passing CI and no unresolved local changes.

### Key Entities *(include if feature involves data)*

- **Current Main Baseline**: The authoritative merged state that all new work must start from.
- **Stale PR Reference**: A prior PR or branch that may contain useful ideas but is not mergeable as-is.
- **Recovery Memo**: The durable artifact at `specs/002-phase6-submit-admission-recovery/recovery-review.md` that records the current-main baseline, stale PR context, and salvage classification.
- **Salvage Classification**: A keep, rewrite, or reject decision for each stale PR concept.
- **Recovery Strategy Review**: An independent pre-implementation review of the salvage plan before code changes begin.
- **Fresh Phase 6 PR**: The new narrow implementation PR created from current `main`.
- **Submit Admission Contract**: The behavior that enforces live-canary order-count and notional limits before order submission.
- **Anti-Slop Cleanup Pass**: A bounded quality pass that removes drift, duplication, dead code, and unsupported abstractions from the fresh work.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The recovery memo classifies 100% of stale PR #317 Phase 6-relevant concepts as keep, rewrite, or reject before implementation begins.
- **SC-002**: At least two independent recovery-strategy reviewer responses evaluate the recovery strategy before fresh Phase 6 implementation begins.
- **SC-003**: The fresh Phase 6 PR diff contains zero files whose only purpose is to restore stale Phase 3-5 behavior already superseded by current `main`.
- **SC-004**: The fresh Phase 6 PR demonstrates the required order of operations: evidence first, submit admission second, order submission third.
- **SC-005**: The fresh Phase 6 PR has passing targeted tests and passing CI at the exact head before external code review is requested.
- **SC-006**: Every substantive recovery-strategy review or code-review comment is resolved, disproved with evidence, or tracked as explicitly out of scope before merge.

## Assumptions

- Current `main` already contains the canonical Phase 3-5 architecture after PR #322.
- Stale PR #317 contains useful Phase 6 submit-admission ideas but is not a safe base for direct continuation.
- The maintainer prefers fewer, narrower PRs with stronger review prompts over a long stacked chain.
- The immediate goal is recovery strategy and Phase 6 preparation, not Phase 7 readiness or Phase 8 live canary execution.
- Recovery-strategy reviewers are most useful before implementation when they receive exact SHAs, explicit scope, and a clear stale-vs-current map.
- Code reviewers are most useful after a fresh Phase 6 PR exists, exact-head CI is green, and local findings are resolved.
