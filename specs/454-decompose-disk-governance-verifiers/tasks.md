# Tasks: Decompose Disk-Governance Verifiers

**Input**: Design documents from `/specs/454-decompose-disk-governance-verifiers/`
**Prerequisites**: `plan.md`, `spec.md`, `evidence.md`, `research.md`, `data-model.md`, `contracts/command-understanding.md`, `quickstart.md`
**Tests**: Required by issue #454 and constitution TDD gate. Characterization/parity tests must be written before extraction.
**Execution Mode**: Sequential only. One bounded #454 PR; do not merge without operator approval.

## Phase 1: Setup

**Purpose**: Establish fresh-main branch hygiene and issue-local Speckit artifacts.

- [x] T001 Verify #375 is closed as completed and #454 is open; record status in `specs/454-decompose-disk-governance-verifiers/evidence.md`.
- [x] T002 Create branch `codex/454-decompose-disk-governance-verifiers` from fresh `origin/main` at merge commit `f354efbaa2afc78575e9cc40580cf2b682bd66e6`.
- [x] T003 Run baseline `python3 scripts/test_rust_verification_cache_retention.py` and record result in `specs/454-decompose-disk-governance-verifiers/evidence.md`.
- [x] T004 Run baseline `python3 scripts/test_verify_ci_workflow_hygiene.py` and record result in `specs/454-decompose-disk-governance-verifiers/evidence.md`.
- [x] T005 Generate #454 Speckit `spec.md`, `plan.md`, `research.md`, `data-model.md`, `contracts/command-understanding.md`, `quickstart.md`, `evidence.md`, and this `tasks.md`.
- [x] T006 Record initial helper-candidate equivalence findings in `specs/454-decompose-disk-governance-verifiers/evidence.md` and `research.md`.

---

## Phase 2: Foundational Gates

**Purpose**: Complete planning review before implementation.

- [x] T007 Run unresolved-marker scan over #454 Speckit docs and verify `AGENTS.md` points at `specs/454-decompose-disk-governance-verifiers/plan.md`.
- [x] T008 Run `git diff --check` on the initial planning changes.
- [x] T009 Request Claude adversarial planning review for `specs/454-decompose-disk-governance-verifiers/` and record verdict in `evidence.md`.
- [x] T010 Request Kimi adversarial planning review for `specs/454-decompose-disk-governance-verifiers/` and record verdict in `evidence.md`.
- [x] T011 Request GLM adversarial planning review for `specs/454-decompose-disk-governance-verifiers/` and record verdict in `evidence.md`.
- [x] T012 Request DeepSeek adversarial planning review for `specs/454-decompose-disk-governance-verifiers/` and record verdict in `evidence.md`.
- [x] T013 Resolve or explicitly classify every source-proven planning blocker in `specs/454-decompose-disk-governance-verifiers/`.
- [x] T014 Verify Claude, Gemini, Kimi, Grok, GLM, and DeepSeek reviewer availability and record readiness in `evidence.md`.
- [x] T015 Re-run exact-head adversarial planning review for Claude, Gemini, Kimi, Grok, GLM, and DeepSeek; record unanimous current-head approval before implementation.

**Checkpoint**: No implementation starts until T007 through T015 are complete.

---

## Phase 3: User Story 1 - Preserve Accepted Command Classification (Priority: P1)

**Goal**: Current representative command-understanding behavior is captured before parser code moves.

**Independent Test**: `python3 scripts/test_command_understanding.py` fails before `scripts/command_understanding.py` exists, then passes once the shared module preserves current classifications.

### Tests for User Story 1

- [x] T016 [US1] Add pre-extraction comparison tests or evidence cases that classify each candidate helper family as equivalent, divergent, or deferred before `scripts/command_understanding.py` exists.
- [x] T017 [P] [US1] Add RED characterization tests for `command_tokens`, shell command boundaries, command substitutions, and Python inline command payloads in `scripts/test_command_understanding.py`.
- [x] T018 [P] [US1] Add RED characterization tests for renamed cargo/rustc executable detection, current wrapper handling, cargo subcommand scanning, and target-routing scan cases in `scripts/test_command_understanding.py`.

### Implementation for User Story 1

- [x] T019 [US1] Add `scripts/command_understanding.py` with mechanically copied current helper behavior only for candidates classified equivalent by T016 through T018.
- [x] T020 [US1] Run `python3 scripts/test_command_understanding.py` and record RED/GREEN evidence in `specs/454-decompose-disk-governance-verifiers/evidence.md`.

**Checkpoint**: User Story 1 is complete when shared-module characterization tests pass and prove the current baseline behavior.

---

## Phase 4: User Story 2 - Share Parser Logic Without New Semantics (Priority: P2)

**Goal**: Both verifier clients use the shared parser path for extracted duplicate helper families.

**Independent Test**: Existing issue-named suites and `scripts/test_command_understanding.py` pass after rewiring both clients.

### Tests for User Story 2

- [x] T021 [P] [US2] Add parity tests in `scripts/test_command_understanding.py` proving both verifier clients classify representative samples consistently through the shared helper path for extracted equivalent helpers.
- [x] T022 [P] [US2] Add drift-guard tests in `scripts/test_command_understanding.py` for representative raw-cargo/storage override and active-process command samples.

### Implementation for User Story 2

- [x] T023 [US2] Rewire `scripts/rust_verification.py` to import extracted equivalent helpers from `scripts/command_understanding.py`.
- [x] T024 [US2] Rewire `scripts/verify_ci_workflow_hygiene.py` to import extracted equivalent helpers from `scripts/command_understanding.py`.
- [x] T025 [US2] Remove duplicate local helper definitions only after both clients import the shared path and tests pass; leave divergent helpers local.
- [x] T026 [US2] Run `python3 scripts/test_command_understanding.py`, `python3 scripts/test_rust_verification_cache_retention.py`, and `python3 scripts/test_verify_ci_workflow_hygiene.py`; record results in `evidence.md`.

**Checkpoint**: User Story 2 is complete when both verifier clients use the shared path with no accepted behavior drift.

---

## Phase 5: User Story 3 - Keep Remaining Verifier Surfaces Reviewable (Priority: P3)

**Goal**: Remaining oversized-surface risk is documented, and any split is mechanical only.

**Independent Test**: Reviewers can inspect `evidence.md` and see what was reduced, what remains, and why deferred work is outside this PR.

### Tests for User Story 3

- [x] T027 [US3] Add or update evidence checks in `scripts/test_command_understanding.py` only if a mechanical test split is made.

### Implementation for User Story 3

- [x] T028 [US3] Update `specs/454-decompose-disk-governance-verifiers/evidence.md` with extracted helper families, divergent candidates, line-count deltas, remaining oversized surfaces, and deferred non-goals.
- [x] T029 [US3] Split any moved characterization tests only if the split is mechanical and reviewable; otherwise record the deferral in `evidence.md`.

**Checkpoint**: User Story 3 is complete when remaining risk is explicit and no cosmetic broad split is hidden in the PR.

---

## Final Phase: Verification, PR, And Review

**Purpose**: Make #454 review-ready without merging.

- [x] T030 Run `python3 -m py_compile scripts/command_understanding.py scripts/test_command_understanding.py scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py`.
- [x] T031 Run `git diff --check`.
- [x] T032 Run unresolved-marker scan over changed docs and scripts.
- [x] T033 Commit and push branch `codex/454-decompose-disk-governance-verifiers`.
- [x] T034 Open the #454 PR with scope, non-goals, evidence map, tests, review status, skipped slots, remaining risk, and stop-before-merge note.
- [ ] T035 Confirm exact-head GitHub CI is green for the #454 PR.
- [ ] T036 Request exact-PR-head external adversarial review from Claude, Gemini, Kimi, Grok, GLM, and DeepSeek; record all verdicts or skipped slots in `evidence.md`.
- [ ] T037 Stop for operator approval before merge.

## Dependencies & Execution Order

- Phase 1 must complete before Phase 2.
- Phase 2 must complete with current-head no-blocker review before implementation.
- User Story 1 must complete before User Story 2.
- User Story 2 must complete before User Story 3.
- Final Phase must complete before #454 is considered review-ready.

## Parallel Opportunities

- T017 and T018 can be drafted independently after T016 classification scope is set.
- T021 and T022 can be drafted independently after T019/T020.
- No implementation phase should run in parallel with another branch or issue.

## Implementation Strategy

1. Finish Phase 2 planning gates and current-head adversarial review.
2. Implement User Story 1 with RED shared-module tests and helper equivalence classification.
3. Mechanically extract only equivalent helper families and rewire both verifier clients for User Story 2.
4. Update evidence and remaining risk for User Story 3.
5. Complete local verification, open PR, wait for exact-head CI, request external review, and stop before merge.
