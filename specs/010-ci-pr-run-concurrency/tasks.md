# Tasks: CI PR Run Concurrency

**Input**: Design documents from `specs/010-ci-pr-run-concurrency/`
**Prerequisites**: `spec.md`, `plan.md`, `research.md`, `data-model.md`, `quickstart.md`

## Phase 1: Setup

- [x] T001 [P] Record #355 scope and runtime evidence requirements in `specs/010-ci-pr-run-concurrency/spec.md`
- [x] T002 [P] Record concurrency design decisions in `specs/010-ci-pr-run-concurrency/research.md`
- [x] T003 [P] Create requirements-quality checklist in `specs/010-ci-pr-run-concurrency/checklists/requirements.md`

## Phase 2: Foundational

- [x] T004 [P] Confirm `.github/workflows/ci.yml` already has top-level PR-only concurrency.
- [x] T005 [P] Confirm `scripts/verify_ci_workflow_hygiene.py` lacks concurrency drift checks.

## Phase 3: User Story 1 - PR-Only Cancellation Policy Is Guarded (Priority: P1)

**Goal**: Verifier rejects missing or weakened PR-only concurrency policy.

**Independent Test**: `python3 scripts/test_verify_ci_workflow_hygiene.py` fails before implementation and passes after verifier support is added.

- [x] T006 [US1] Add failing concurrency verifier self-tests in `scripts/test_verify_ci_workflow_hygiene.py`
- [x] T007 [US1] Implement top-level concurrency extraction and invariant checks in `scripts/verify_ci_workflow_hygiene.py`
- [x] T008 [US1] Run local verifier gates from `specs/010-ci-pr-run-concurrency/quickstart.md`

## Phase 4: User Story 2 - Runtime Cancellation Evidence Is Recorded (Priority: P1)

**Goal**: #355 closure is backed by real GitHub Actions run evidence, not only static workflow shape.

**Independent Test**: Issue/PR evidence names exact run IDs, SHAs, conclusions, and newest-head checks.

- [x] T009 [US2] Capture real superseded PR-run evidence with exact run IDs and SHAs
- [ ] T010 [US2] Capture newest-head required-check status after the PR exists
- [ ] T011 [US2] Update issue #355 or PR body with evidence and residuals

## Phase 5: Polish & Cross-Cutting

- [x] T012 Run `git diff --check`
- [ ] T013 Request external reviews after exact-head CI is green
- [ ] T014 Keep PR mapped to #355 only; do not claim #333 or #203 closure

## Dependencies

- T006 before T007.
- T008 after T006-T007.
- T009-T011 after PR CI exists.
- T013 after exact-head CI is green.

## Parallel Opportunities

- T001-T003 are independent docs tasks.
- T009 and T010 can be collected from GitHub once the PR exists and CI starts.

## Implementation Strategy

TDD first: write negative verifier tests for missing/wrong concurrency, watch them fail, implement minimal verifier support, then verify locally and with exact-head CI.
