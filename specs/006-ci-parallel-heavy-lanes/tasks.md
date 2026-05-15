# Tasks: CI Parallel Heavy Lanes

**Input**: Design documents from `specs/006-ci-parallel-heavy-lanes/`
**Prerequisites**: `spec.md`, `plan.md`, `research.md`, `data-model.md`, `quickstart.md`

## Setup

- [x] T001 [P] Fetch and record live #332 issue body/comment and #333 epic acceptance in `specs/006-ci-parallel-heavy-lanes/spec.md`
- [x] T002 [P] Create requirements-quality checklist in `specs/006-ci-parallel-heavy-lanes/checklists/requirements.md`
- [x] T003 [P] Record #332 design decisions in `specs/006-ci-parallel-heavy-lanes/research.md`

## Foundational TDD

- [x] T004 Add failing verifier self-tests for top-level `check-aarch64` job requirements in `scripts/test_verify_ci_workflow_hygiene.py`
- [x] T005 Add failing verifier self-tests for four-way `test` matrix requirements in `scripts/test_verify_ci_workflow_hygiene.py`
- [x] T006 Add failing verifier self-tests for shard reproduction command and shard-aware bounded cache key in `scripts/test_verify_ci_workflow_hygiene.py`
- [x] T007 Add failing verifier self-tests proving `clippy` no longer owns aarch64 compiler install or `just check-aarch64` in `scripts/test_verify_ci_workflow_hygiene.py`
- [x] T008 Add failing justfile passthrough evidence for `just test -- --partition count:1/4`

## User Story 1 - Split Serialized Heavy Checks (Priority: P1)

**Goal**: Host clippy and aarch64 check are independent top-level jobs with independent cache keys and gate checks.

**Independent Test**: `python3 scripts/test_verify_ci_workflow_hygiene.py` fails when `check-aarch64` job, setup/cache, gate need, or result check is missing.

- [x] T009 [US1] Add `check-aarch64` to required job, gate, deploy, and target-dir verifier contracts in `scripts/verify_ci_workflow_hygiene.py`
- [x] T010 [US1] Add top-level `check-aarch64` job in `.github/workflows/ci.yml`
- [x] T011 [US1] Remove aarch64 compiler install and `just check-aarch64` from host `clippy` job in `.github/workflows/ci.yml`
- [x] T012 [US1] Give host `clippy` and `check-aarch64` independent explicit rust-cache keys in `.github/workflows/ci.yml`
- [x] T013 [US1] Extend `just ci-lint-workflow` awk checks for `check-aarch64` setup/cache/gate invariants in `justfile`

## User Story 2 - Shard Full Nextest Deterministically (Priority: P1)

**Goal**: Full nextest runs as four deterministic matrix shards through managed `just test` with one aggregate gate result.

**Independent Test**: Verifier self-tests fail if the `test` job lacks shard values 1-4, `fail-fast: false`, the exact partition command, shard-aware cache key, or reproduction log.

- [x] T014 [US2] Add variadic passthrough args to `managed-test` and `test` recipes in `justfile`
- [x] T015 [US2] Add `strategy.fail-fast: false` and `strategy.matrix.shard: [1, 2, 3, 4]` to the workflow `test` job
- [x] T016 [US2] Change workflow `test` command to `just test -- --partition count:${{ matrix.shard }}/4`
- [x] T017 [US2] Add shard reproduction log output to the workflow `test` job
- [x] T018 [US2] Add bounded shard-aware nextest cache key to the workflow `test` job
- [x] T019 [US2] Enforce matrix, partition, fail-fast, reproduction log, and shard cache invariants in `scripts/verify_ci_workflow_hygiene.py`
- [x] T020 [US2] Extend `just ci-lint-workflow` awk checks for the test-shard invariants in `justfile`

## User Story 3 - Preserve Source-Fence Ownership (Priority: P1)

**Goal**: #342 source-fence filters remain intentionally duplicated in full nextest and required before the sharded `test` lane.

**Independent Test**: Workflow comments and PR body state the duplicate ownership decision; verifier keeps `test needs source-fence` and `gate needs source-fence`.

- [x] T021 [US3] Update `.github/workflows/ci.yml` comments to state source-fence duplicate execution is intentional under #332
- [x] T022 [US3] Preserve `test.needs: [detector, source-fence]` and existing source-fence gate result checks in verifier and workflow
- [ ] T023 [US3] Add PR-body evidence note placeholder for source-fence ownership decision

## User Story 4 - Keep Narrow Lint Ownership (Priority: P2)

**Goal**: Lint extension covers #332 topology only and does not absorb generic #203 or later child work.

**Independent Test**: Spec and PR body name #195, #205, #344, and #340 as not implemented; verifier does not invent their future topology.

- [x] T024 [US4] Ensure verifier changes do not require #195 cache persistence, #205 same-SHA reuse, #344 pass-stub, or #340 config path changes
- [ ] T025 [US4] Update PR body with exact residual scope and before/after evidence placeholders

## Verification

- [x] T026 Run `python3 scripts/test_verify_ci_workflow_hygiene.py`
- [x] T027 Run `python3 scripts/verify_ci_workflow_hygiene.py`
- [x] T028 Run `just ci-lint-workflow`
- [x] T029 Run `just --dry-run test -- --partition count:1/4`
- [x] T030 Run `just fmt-check`
- [x] T031 Run `git diff --check`
- [ ] T032 Obtain exact-head CI run for the final PR head before external review or mark it explicitly blocked by stacked PR trigger limits

## Dependencies

- T004-T008 before implementation tasks.
- T009 before T010-T013 can be considered complete.
- T014 before T016 and T029.
- T015-T018 before T019-T020 can pass.
- T021-T023 after the workflow test topology is known.
- T026-T032 after implementation tasks.

## Parallel Opportunities

- T004-T007 can be drafted together in the verifier self-test file.
- T010-T012 touch workflow clippy/aarch64 topology and can proceed before test matrix edits.
- T014 touches justfile recipe passthrough and can proceed before workflow test command changes.
- T021 can be drafted while T019-T020 verifier work is underway.

## Implementation Strategy

Start with failing self-tests for the exact #332 invariants, then implement the verifier and workflow changes in small groups: check-aarch64 split first, managed test passthrough second, matrix sharding third, and narrow justfile lint extension last. Preserve source-fence duplication explicitly and do not claim exact timing until final-head CI exists.
