# Tasks: CI Workflow Hygiene

**Input**: Design documents from `specs/005-ci-workflow-hygiene/`
**Prerequisites**: `spec.md`, `plan.md`, `research.md`, `data-model.md`, `quickstart.md`

## Phase 1: Setup

- [x] T001 [P] Record current #203 evidence in `specs/005-ci-workflow-hygiene/research.md`
- [x] T002 [P] Create requirements-quality checklist in `specs/005-ci-workflow-hygiene/checklists/requirements.md`

## Phase 2: Foundational

- [x] T003 Add failing topology-verifier self-tests in `scripts/test_verify_ci_workflow_hygiene.py`
- [x] T004 Implement standard-library workflow parser and invariant checks in `scripts/verify_ci_workflow_hygiene.py`
- [x] T005 Wire `scripts/test_verify_ci_workflow_hygiene.py` and `scripts/verify_ci_workflow_hygiene.py` into `just ci-lint-workflow`

## Phase 3: User Story 1 - Workflow Topology Lint Is Explicit (Priority: P1)

**Goal**: Required jobs, needs, and gate checks fail with actionable errors when missing.

**Independent Test**: `python3 scripts/test_verify_ci_workflow_hygiene.py` fails on missing job, missing gate need, missing gate result, and missing deploy direct need fixtures.

- [x] T006 [US1] Enforce exact required job ids in `scripts/verify_ci_workflow_hygiene.py`
- [x] T007 [US1] Enforce `gate.needs` and gate result checks in `scripts/verify_ci_workflow_hygiene.py`
- [x] T008 [US1] Enforce source-fence/test/build direct needs in `scripts/verify_ci_workflow_hygiene.py`

## Phase 4: User Story 2 - Setup Work Is Lane-Specific (Priority: P1)

**Goal**: Managed target-dir resolution is opt-in and only target-cache jobs opt in.

**Independent Test**: Hygiene self-tests fail when a target-cache job lacks opt-in or a non-target-cache job opts in.

- [x] T009 [US2] Add `include-managed-target-dir` input and conditional target-dir step in `.github/actions/setup-environment/action.yml`
- [x] T010 [US2] Set `include-managed-target-dir: "true"` only on target-cache jobs in `.github/workflows/ci.yml`
- [x] T011 [US2] Enforce target-dir opt-in contract in `scripts/verify_ci_workflow_hygiene.py`

## Phase 5: User Story 3 - Detector And Deploy Semantics Are Preserved (Priority: P1)

**Goal**: Remove unnecessary fmt-check serialization while adding deploy direct-needs defense.

**Independent Test**: `just ci-lint-workflow` passes only when fmt-check has no detector need, build remains detector-gated, and deploy directly needs required lanes.

- [x] T012 [US3] Remove `fmt-check` detector dependency in `.github/workflows/ci.yml`
- [x] T013 [US3] Add direct deploy needs for detector, fmt-check, deny, clippy, source-fence, and test in `.github/workflows/ci.yml`
- [x] T014 [US3] Enforce fmt-check/build/deploy semantics in `scripts/verify_ci_workflow_hygiene.py`

## Phase 6: Polish & Cross-Cutting

- [x] T015 Run `python3 scripts/test_verify_ci_workflow_hygiene.py`
- [x] T016 Run `python3 scripts/verify_ci_workflow_hygiene.py`
- [x] T017 Run `just ci-lint-workflow`
- [x] T018 Run `just fmt-check`
- [x] T019 Run `git diff --check`
- [x] T020 Update PR body with exact-head CI and residual #332/#195/#205/#344/#340 scope
- [x] T021 [P] Document accepted verification-support co-scope in `specs/005-ci-workflow-hygiene/spec.md`
- [x] T022 [P] Serialize LiveNode-heavy tests in `tests/lake_batch.rs`, `tests/nt_runtime_capture.rs`, and `tests/platform_runtime.rs`
- [x] T023 [P] Extend pure-Rust verifier alias detection in `scripts/verify_bolt_v3_pure_rust_runtime.py` and `scripts/test_verify_bolt_v3_pure_rust_runtime.py`
- [ ] T024 [P] Re-record exact-head verification for the spec-kit co-scope traceability fix after commit/push

## Dependencies

- T003 before T004.
- T004 before T005.
- T006-T008 depend on T004.
- T009-T011 depend on T003-T004.
- T012-T014 depend on T003-T004.
- T015-T020 after implementation tasks.
- T021-T024 after accepted co-scope is identified and before final review status.

## Parallel Opportunities

- T001 and T002 can run in parallel.
- T006-T008 can be implemented together after parser foundation exists.
- T009 and T012/T013 touch different files and can proceed after tests define the contract.

## Implementation Strategy

Implement one TDD tracer first: missing deploy direct needs must fail in `scripts/test_verify_ci_workflow_hygiene.py`, then implement the verifier. Add the target-dir opt-in and fmt-check detector checks after the parser path is green.
