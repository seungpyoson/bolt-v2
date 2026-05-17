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
- [x] T013 [US3] Add direct deploy needs for detector, fmt-check, deny, clippy, check-aarch64, source-fence, and test in `.github/workflows/ci.yml`
- [x] T014 [US3] Enforce fmt-check/build/deploy semantics in `scripts/verify_ci_workflow_hygiene.py`

## Phase 6: Polish & Cross-Cutting

- [x] T015 Run `python3 scripts/test_verify_ci_workflow_hygiene.py`
- [x] T016 Run `python3 scripts/verify_ci_workflow_hygiene.py`
- [x] T017 Run `just ci-lint-workflow`
- [x] T018 Run `just fmt-check`
- [x] T019 Run `git diff --check`
- [x] T020 Update PR body with exact-head CI, landed #332 base-topology note, and residual #195/#205/#344/#340 scope

## Phase 7: Prebuilt CI Tool Install Contract

- [x] T021 [P] Add failing verifier self-tests for source-built `cargo-deny`, `cargo-nextest`, and `cargo-zigbuild` regressions in `scripts/test_verify_ci_workflow_hygiene.py`
- [x] T022 [P] Switch CI/advisory Rust helper tool installs to prebuilt paths in `.github/workflows/ci.yml` and `.github/workflows/advisory.yml`
- [x] T023 Add pinned `cargo-zigbuild` Linux x86_64 archive SHA256 to `justfile`
- [x] T024 Export `zigbuild_x86_64_unknown_linux_gnu_sha256` from `.github/actions/setup-environment/action.yml`
- [x] T025 Enforce install-action pinning, `fallback: none`, source-install rejection, full cargo-zigbuild install steps, and pinned SHA256 use in `scripts/verify_ci_workflow_hygiene.py`
- [x] T026 Update `spec.md`, `quickstart.md`, `data-model.md`, `research.md`, `plan.md`, and checklist docs for the prebuilt install contract

## Dependencies

- T003 before T004.
- T004 before T005.
- T006-T008 depend on T004.
- T009-T011 depend on T003-T004.
- T012-T014 depend on T003-T004.
- T015-T020 after implementation tasks.

## Parallel Opportunities

- T001 and T002 can run in parallel.
- T006-T008 can be implemented together after parser foundation exists.
- T009 and T012/T013 touch different files and can proceed after tests define the contract.
- T021 and T022 can run after T004 because the verifier parser already exists.

## Implementation Strategy

Implement one TDD tracer first: missing deploy direct needs must fail in `scripts/test_verify_ci_workflow_hygiene.py`, then implement the verifier. Add the target-dir opt-in and fmt-check detector checks after the parser path is green.
