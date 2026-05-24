# Feature Specification: #466 Disk-Governance Verifier Decomposition

**Feature Branch**: `goal/466-disk-governance-verifier-decomposition`  
**Created**: 2026-05-24  
**Status**: Draft  
**Input**: User description: "Complete issue #466 end to end: continue disk-governance verifier decomposition after cargo scanner extraction until every remaining #466 scope item is resolved."

## User Scenarios & Testing

### User Story 1 - Govern The Full #466 Scope Ledger (Priority: P1)

As the operator, I need a single evidence ledger that preserves every remaining #466 scope item so no partial helper slice can be mistaken for issue completion.

**Why this priority**: This prevents repeat closure mistakes and defines the source of truth for all later PR slices.

**Independent Test**: Review `evidence.md` and confirm every #466 item has current-main runtime evidence, static verifier evidence, test/doc evidence, verdict, chosen resolution, touched files, required tests, review evidence, and final state.

**Acceptance Scenarios**:

1. **Given** current `origin/main` after PR #465, **When** the #466 ledger is reviewed, **Then** all eight remaining scope items from issue #466 are present and traceable to current-main evidence.
2. **Given** a PR covers only one helper family, **When** the PR body and ledger are checked, **Then** #466 remains open unless every ledger item is resolved or explicitly operator-moved.
3. **Given** a ledger item is intentionally kept local, **When** the final ledger is checked, **Then** it has characterization evidence and an explicit reason.

---

### User Story 2 - Reduce Runtime/Static Drift Without New Semantics (Priority: P2)

As the operator, I need each decomposition slice to reduce duplicated verifier drift risk only where characterization proves equivalence, while preserving accepted disk-governance behavior.

**Why this priority**: Incorrect extraction can make runtime enforcement and static CI/no-mistakes hygiene disagree.

**Independent Test**: For each implemented slice, run focused RED/GREEN characterization or parity tests, then the relevant verifier suites, and confirm changed helpers match the ledger resolution.

**Acceptance Scenarios**:

1. **Given** a helper family is proven equivalent, **When** it is extracted or rewired, **Then** both verifier clients use the shared path and parity tests fail if either client drifts.
2. **Given** a helper family is divergent or insufficiently proven, **When** the implementation completes, **Then** it remains local with tests documenting the current boundary.
3. **Given** file splitting or test import cleanup is performed, **When** the relevant suites run, **Then** behavior and direct-script/module import coverage remain unchanged.

---

### User Story 3 - Gate PRs And Issue Closure With Evidence (Priority: P3)

As the operator, I need every PR slice and final issue closure to be gated by local verification, exact-head CI, external review, and explicit operator approval.

**Why this priority**: #466 is multi-PR capable; completion cannot be inferred from tests, one merge, or moved work.

**Independent Test**: For each PR-ready slice, inspect the PR body, ledger, CI state, external review records, and issue comments before merge or closure.

**Acceptance Scenarios**:

1. **Given** a PR is ready for review, **When** external review records are checked, **Then** Claude, Gemini, Grok, GLM, DeepSeek, and Kimi have usable verdicts or an explicit operator waiver for a failed/skipped slot.
2. **Given** a PR is merged, **When** work resumes, **Then** the branch returns to current `main` and continues from unresolved #466 ledger items, not stale feature-branch proof.
3. **Given** every ledger item is resolved, **When** final completion is requested, **Then** final whole-#466 verification and external review evidence exist before asking operator approval to close #466.

### Edge Cases

- A reviewer slot times out, fails, or returns no verdict; it is recorded as failed/skipped, not approval, unless the operator explicitly waives it.
- A helper family looks similar but has different return shape, filesystem boundary, input normalization, or policy context; it remains local unless evidence proves a safe shared primitive.
- A mechanical split changes import mode, direct-script behavior, or execution order; the split is rejected unless tests prove preservation.
- Work is moved to another issue; #466 is not complete unless the operator explicitly approves the scope movement and #466 is updated.

## Requirements

### Functional Requirements

- **FR-001**: The #466 evidence ledger MUST track command tokenization and line-boundary tokenization.
- **FR-002**: The #466 evidence ledger MUST track shell command substitution parsing.
- **FR-003**: The #466 evidence ledger MUST track renamed `cargo` and `rustc` detection.
- **FR-004**: The #466 evidence ledger MUST track wrapper handling.
- **FR-005**: The #466 evidence ledger MUST track target-routing override policy beyond the pure cargo scan helper from PR #465.
- **FR-006**: The #466 evidence ledger MUST track mechanical splitting of oversized verifier and verifier-test files by concern where behavior-preserving and reviewable.
- **FR-007**: The #466 evidence ledger MUST track test-only import setup cleanup without weakening direct-script versus module import coverage.
- **FR-008**: The #466 evidence ledger MUST track static `consume_cargo_global_options` drift risk, including `CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT`.
- **FR-009**: Each ledger item MUST record runtime implementation evidence, static verifier evidence, test/doc evidence, equivalence verdict, chosen resolution, files touched or intentionally untouched, tests required, review evidence, and final state.
- **FR-010**: Implementation MUST start each moved or generalized behavior with characterization or parity evidence before production code changes.
- **FR-011**: Extracted helpers MUST be shared only when equivalence is proven across required verifier clients.
- **FR-012**: Divergent or unproven helper behavior MUST stay local with characterization evidence unless the operator explicitly approves semantic change.
- **FR-013**: Each PR slice MUST state #466 ledger items covered, non-goals, behavior-preservation strategy, tests, review results, residual risk, and whether #466 remains open.
- **FR-014**: Pre-implementation and post-implementation review gates MUST require Claude, Gemini, Grok, GLM, DeepSeek, and Kimi approval or explicit operator waiver for failed/skipped slots.
- **FR-015**: #466 MUST NOT be closed or declared complete until all ledger items are resolved, all needed implementation PRs are merged, final verification passes, final external review is clean or waived, and the operator explicitly approves closure.

### Key Entities

- **Scope Ledger Item**: One #466 residual work item with evidence, verdict, chosen resolution, verification, review, and final state.
- **Helper Family**: A command-understanding, wrapper, target-routing, or option-handling behavior that may be extracted, kept local, split, or cleaned up.
- **PR Slice**: A bounded implementation PR that resolves one coherent helper family or mechanical cleanup without claiming broader completion.
- **Review Gate**: Required reviewer verdict set plus operator waivers where applicable.
- **Verification Evidence**: Local command output, exact-head CI state, characterization RED/GREEN record, and final whole-issue evidence.

## Success Criteria

### Measurable Outcomes

- **SC-001**: The final #466 ledger has zero rows marked open, insufficient evidence, unreviewed, partial, tracked later, or TBD.
- **SC-002**: Every extracted helper family has tests proving both verifier clients use the shared path and preserve current behavior.
- **SC-003**: Every kept-local helper family has characterization evidence explaining why extraction was not performed.
- **SC-004**: Every implementation PR needed for #466 has passing relevant local verification, exact-head CI, and external review evidence or explicit waiver records.
- **SC-005**: #466 remains open across intermediate PR slices and is closed only after explicit operator approval with final completion evidence.

## Assumptions

- Current `origin/main` after PR #465 is authoritative for all evidence and implementation.
- Multiple PR slices are acceptable, but each slice must be bounded and must not claim full #466 completion unless the final ledger proves it.
- DeepSeek and GLM source sends have standing operator approval, but approval metadata and audit evidence remain required.
- No new shell semantics, wrapper families, cargo behavior, or command-prediction behavior are in scope without explicit operator approval.
