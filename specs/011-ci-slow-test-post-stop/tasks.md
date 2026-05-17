# Tasks: CI Slow Test Post-Stop Delay

**Input**: Design documents from `specs/011-ci-slow-test-post-stop/`

## Phase 1: Evidence

- [x] T001 Capture #333/#357 current scope from live GitHub issues.
- [x] T002 Run representative baseline test and record 10s post-stop delay.
- [x] T003 Inspect target files for post-stop/drain assertions before editing.

## Phase 2: TDD Implementation

- [x] T004 Add shared `fast_test_live_node` helper with zero post-stop delay.
- [x] T005 Replace plain `LiveNode::builder(...).build()` sites in `venue_contract`, `nt_runtime_capture`, and `lake_batch`.
- [x] T006 Re-run representative test and record sub-second runtime.

## Phase 3: Verification

- [x] T007 Run full targeted test binaries: `venue_contract`, `nt_runtime_capture`, `lake_batch`.
- [x] T008 Run formatting/diff checks.
- [ ] T009 Push branch, open PR for #357 only, and capture exact-head CI.
- [ ] T010 Request no-mistakes and external reviews after exact-head CI is green.
