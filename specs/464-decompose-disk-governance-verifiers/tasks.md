# Tasks: #464 Cargo Scanner Helper Decomposition

**Input**: Design documents from `specs/464-decompose-disk-governance-verifiers/`
**Prerequisites**: `spec.md`, `plan.md`, `research.md`, `data-model.md`, `contracts/cargo-scanner.md`, `quickstart.md`, `evidence.md`
**Tests**: Required. Characterization/parity tests must be written and observed failing before moving cargo scanner code.
**Execution Mode**: Sequential. One bounded PR for issue #464; do not merge without operator approval.

## Phase 1: Setup And Evidence

**Purpose**: Establish current-main branch hygiene and issue-local Speckit artifacts.

- [x] T001 Verify branch `codex/464-verifier-decomposition` starts from `origin/main` commit `817ddfc9af8cd835ee6143f0562595f73a1d2645` in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [x] T002 Record issue #464, issue #454, and PR #461 current state in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [x] T003 Run baseline `python3 scripts/test_command_understanding.py` and record result in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [x] T004 Run baseline `python3 scripts/test_rust_verification_cache_retention.py` and record result in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [x] T005 Run baseline `python3 scripts/test_verify_ci_workflow_hygiene.py` and record result in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [x] T006 Create #464 Speckit `spec.md`, `plan.md`, `research.md`, `data-model.md`, `contracts/cargo-scanner.md`, `quickstart.md`, `evidence.md`, and `tasks.md`.
- [x] T007 Classify issue #464 helper candidates in `specs/464-decompose-disk-governance-verifiers/research.md`.

## Phase 2: Foundational Review Gates

**Purpose**: Complete planning checks and external adversarial review before implementation.

- [x] T008 Run unresolved-marker scan over `specs/464-decompose-disk-governance-verifiers/`.
- [x] T009 Run `git diff --check` on current planning changes including `specs/464-decompose-disk-governance-verifiers/tasks.md`.
- [ ] T010 Request Claude adversarial planning review for `specs/464-decompose-disk-governance-verifiers/` and record verdict in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T011 Request Gemini adversarial planning review for `specs/464-decompose-disk-governance-verifiers/` and record verdict in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T012 Request Kimi adversarial planning review for `specs/464-decompose-disk-governance-verifiers/` and record verdict in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T013 Request Grok adversarial planning review for `specs/464-decompose-disk-governance-verifiers/` and record verdict in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T014 Request GLM adversarial planning review for `specs/464-decompose-disk-governance-verifiers/`, record approval-request metadata, and record verdict in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T015 Request DeepSeek adversarial planning review for `specs/464-decompose-disk-governance-verifiers/`, record approval-request metadata, and record verdict in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T016 Resolve or explicitly record every planning review finding in `specs/464-decompose-disk-governance-verifiers/evidence.md`.

**Checkpoint**: No implementation starts until T008 through T016 pass and every available reviewer has approved or the operator explicitly waives a failed/skipped slot.

## Phase 3: User Story 1 - Characterize Cargo Scanner Behavior (Priority: P1)

**Goal**: Focused tests fail before shared cargo scanner exports exist, then prove runtime/static/shared parity after extraction.

**Independent Test**: `python3 scripts/test_command_understanding.py` fails before shared exports exist and passes after both verifier clients use the shared helper family.

- [ ] T017 [US1] RED: Add shared cargo scanner parity tests in `scripts/test_command_understanding.py` for `cargo_subcommand_with_index`, static `start` offset, `cargo_subcommand`, `nextest_subcommand_with_index`, and `cargo_args_for_target_routing_scan`.
- [ ] T018 [US1] RED: Run `python3 scripts/test_command_understanding.py` and record the expected missing-export failure in `specs/464-decompose-disk-governance-verifiers/evidence.md`.

## Phase 4: User Story 2 - Share Cargo Scanner Helpers (Priority: P2)

**Goal**: Runtime and static verifier clients import one shared cargo scanner helper family without policy changes.

**Independent Test**: `python3 scripts/test_command_understanding.py`, `python3 scripts/test_rust_verification_cache_retention.py`, and `python3 scripts/test_verify_ci_workflow_hygiene.py` pass.

- [ ] T019 [US2] GREEN: Add cargo scanner option constants and helper functions in `scripts/command_understanding.py`.
- [ ] T020 [US2] GREEN: Rewire `scripts/rust_verification.py` to import cargo scanner helpers from `scripts/command_understanding.py` and remove duplicate local helper definitions.
- [ ] T021 [US2] GREEN: Rewire `scripts/verify_ci_workflow_hygiene.py` to import cargo scanner helpers from `scripts/command_understanding.py` and remove duplicate local helper definitions.
- [ ] T022 [US2] GREEN: Run `python3 scripts/test_command_understanding.py` and record pass evidence in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T023 [US2] GREEN: Run `python3 scripts/test_rust_verification_cache_retention.py` and record pass evidence in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T024 [US2] GREEN: Run `python3 scripts/test_verify_ci_workflow_hygiene.py` and record pass evidence in `specs/464-decompose-disk-governance-verifiers/evidence.md`.

## Phase 5: User Story 3 - Record Residual Scope (Priority: P3)

**Goal**: The PR documents what changed, what remained local, and why broader #464 work remains.

**Independent Test**: `specs/464-decompose-disk-governance-verifiers/evidence.md` and the PR body state selected scope and residual scope without claiming to close all #464 work.

- [ ] T025 [US3] Update `specs/464-decompose-disk-governance-verifiers/evidence.md` with implementation result, line references, and residual #464 scope.
- [ ] T026 [US3] Update `specs/464-decompose-disk-governance-verifiers/research.md` with final classification and verification results.

## Final Phase: Verification, PR, Review, Merge Gate

**Purpose**: Prove exact-head behavior preservation and stop for operator approval before merge.

- [ ] T027 Run `python3 -m py_compile scripts/command_understanding.py scripts/test_command_understanding.py scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py`.
- [ ] T028 Run `git diff --check`.
- [ ] T029 Run `just ci-lint-workflow`.
- [ ] T030 Run unresolved-marker scan over `specs/464-decompose-disk-governance-verifiers/` and touched Python files.
- [ ] T031 Commit branch `codex/464-verifier-decomposition` with `scripts/command_understanding.py`, `scripts/rust_verification.py`, `scripts/verify_ci_workflow_hygiene.py`, `scripts/test_command_understanding.py`, and `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T032 Push branch `codex/464-verifier-decomposition` containing `specs/464-decompose-disk-governance-verifiers/tasks.md`.
- [ ] T033 Open PR for issue #464 with scope, non-goals, evidence map, chosen slice, remaining local behavior, tests, external review results, skipped review slots, residual risk, and relationship to PR #461 from `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T034 Confirm exact-head GitHub CI is green for the PR head that includes `scripts/command_understanding.py`.
- [ ] T035 Request exact-head implementation review from Claude, Gemini, Kimi, Grok, GLM, and DeepSeek; record all verdicts or skipped/failed slots in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T036 Resolve or explicitly record every implementation review finding in `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T037 Stop for explicit operator merge approval before merging the PR described by `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T038 After explicit merge approval and verification, merge with a normal merge commit unless the operator explicitly asks for squash merge; record final relationship to `specs/464-decompose-disk-governance-verifiers/evidence.md`.
- [ ] T039 Confirm remaining #464 decomposition scope is explicitly tracked in issue #464 or a named follow-up issue before claiming completion in `specs/464-decompose-disk-governance-verifiers/evidence.md`.

## Dependencies & Execution Order

- Phase 1 must complete before Phase 2.
- Phase 2 must complete with current-head planning approval before implementation.
- User Story 1 must complete before User Story 2.
- User Story 2 must complete before User Story 3.
- Final Phase must complete before merge readiness.

## Parallel Opportunities

- T010 through T015 can run in parallel after T008 and T009 pass.
- T022 through T024 must run after T019 through T021.
- External implementation reviews in T035 can run in parallel after exact-head local verification and PR push.

## Implementation Strategy

1. Finish planning checks and unanimous planning review.
2. Add failing shared cargo scanner tests.
3. Mechanically move selected cargo scanner helpers into `scripts/command_understanding.py`.
4. Rewire both verifier clients to import the shared helpers.
5. Run local verifier gates, push, open PR, confirm CI, request external implementation review, and stop for operator approval.
