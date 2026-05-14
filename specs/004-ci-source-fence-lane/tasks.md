# Tasks: CI Source-Fence Lane

**Input**: `specs/004-ci-source-fence-lane/spec.md`
**Scope**: #342 only. Add the early source-fence/verifier lane and required narrow linter/gate invariants. Do not implement #332 sharding or other #333 children.

## Phase 1: Setup

- [x] T001 Create #342 spec-kit artifacts in `specs/004-ci-source-fence-lane/`
- [x] T002 Re-read live #342, #333, #332, and #203 issue bodies and record cross-issue constraints in `specs/004-ci-source-fence-lane/spec.md`

## Phase 2: TDD Red

- [x] T003 [US3] Add narrow #342 linter expectations in `justfile` before workflow changes
- [x] T004 [US3] Run `just ci-lint-workflow` and capture the expected failure for missing `source-fence` topology

## Phase 3: Verifier Script Set

- [x] T005 [US2] Add `scripts/verify_bolt_v3_pure_rust_runtime.py`
- [x] T006 [US2] Add `scripts/verify_bolt_v3_status_map_current.py`
- [x] T007 [US2] Make `scripts/verify_bolt_v3_naming.py` deterministic without ambient unpinned Python packages if required
- [x] T008 [US2] Update `docs/bolt-v3/2026-04-28-source-grounded-status-map.md` row 3 to cite the pure-Rust runtime verifier

## Phase 4: Source-Fence Recipe And Workflow

- [x] T009 [US1] Add `source-fence` recipe in `justfile` running all six verifier scripts and both canonical cargo test filters
- [x] T010 [US1] Add top-level `source-fence` job in `.github/workflows/ci.yml`
- [x] T011 [US1] Make `test` depend on `source-fence` in `.github/workflows/ci.yml`
- [x] T012 [US3] Add `source-fence` to `gate.needs` and require `needs.source-fence.result == "success"`
- [x] T013 [US1] Document temporary duplicate source-fence test execution until #332 resolves ownership

## Phase 5: Verification

- [x] T014 [US3] Re-run `just ci-lint-workflow` and confirm the red linter now passes
- [x] T015 [US2] Run `python3 scripts/verify_bolt_v3_runtime_literals.py`
- [x] T016 [US2] Run `python3 scripts/verify_bolt_v3_provider_leaks.py`
- [x] T017 [US2] Run `python3 scripts/verify_bolt_v3_core_boundary.py`
- [x] T018 [US2] Run `python3 scripts/verify_bolt_v3_naming.py`
- [x] T019 [US2] Run `python3 scripts/verify_bolt_v3_status_map_current.py`
- [x] T020 [US2] Run `python3 scripts/verify_bolt_v3_pure_rust_runtime.py`
- [x] T021 [US1] Run `just source-fence`
- [x] T022 [US1] Prove a deliberate stale source-fence assertion fails through `just source-fence`, then revert the temporary mutation
- [x] T023 Run `git diff --check`
- [ ] T024 Push branch, open stacked PR, and collect exact-head CI evidence

## Dependencies

- T003 before T004 and before workflow implementation.
- T005-T008 before T009.
- T009 before T010.
- T010-T013 before T014.
- T014-T023 before push and CI evidence.

## MVP

Complete #342 with one required `source-fence` lane, one local recipe, complete verifier script list, fail-closed gate behavior, and exact-head CI evidence.
