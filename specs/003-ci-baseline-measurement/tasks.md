# Tasks: CI Baseline Measurement

**Input**: `specs/003-ci-baseline-measurement/spec.md`
**Scope**: #343 only. Measure current CI behavior and link the baseline. Do not change workflow topology.

## Phase 1: Setup

- [x] T001 Create #343 spec-kit artifacts in `specs/003-ci-baseline-measurement/`
- [x] T002 Create durable baseline document in `docs/ci/ci-baseline-2026-05-15.md`

## Phase 2: Evidence Collection

- [x] T003 [P] Fetch live #333 and #343 issue body/comment state from GitHub
- [x] T004 [P] Fetch all #333 child issue body/comment states from GitHub
- [x] T005 [P] Fetch current representative PR run metadata for run `25855655415`
- [x] T006 [P] Fetch current build-affecting PR run metadata for run `25866930064`
- [x] T007 [P] Fetch current main-push run metadata for run `25862551803`
- [x] T007a [P] Refresh exact-base main-push run metadata and cache evidence for run `25866346320`
- [x] T008 [P] Fetch smoke-tag run metadata for run `24623274722`
- [x] T008a [P] Fetch same-SHA main-push metadata and cache evidence for run `24623219988`
- [x] T009 [P] Fetch source-fence failure run metadata for run `25859831755`
- [x] T010 [P] Fetch targeted cache and test/build log excerpts for `test`, `clippy`, and `build` jobs where relevant

## Phase 3: Baseline Artifact

- [x] T011 [US1] Compute workflow wall time, job wall time, raw runner minutes, and rounded runner-minute estimate in `docs/ci/ci-baseline-2026-05-15.md`
- [x] T012 [US1] Record cache warmth and compile/test/build observations in `docs/ci/ci-baseline-2026-05-15.md`
- [x] T013 [US1] Identify critical path per run shape in `docs/ci/ci-baseline-2026-05-15.md`
- [x] T014 [US2] Map all nine #333 child issues to current state, scope owner, dependency, baseline consumer, and issue-body requirement inventory in `docs/ci/ci-baseline-2026-05-15.md`
- [x] T014a [US2] Record the live-source conflict between #344 and #333/#335 about drift-detection lint scope in `docs/ci/ci-baseline-2026-05-15.md`

## Phase 4: Checks And Linkage

- [x] T015 Run `rg -n "25855655415|25866930064|25866346320|25859831755|25862551803|24623219988|24623274722|#343|#342|#332|#195|#205|#203|#335|#344|#340|#333|drift-detection" docs/ci/ci-baseline-2026-05-15.md specs/003-ci-baseline-measurement`
- [x] T016 Run `git diff --check`
- [x] T017 Run `just ci-lint-workflow`
- [x] T017a Run `test -z "$(git diff --name-only origin/main...HEAD -- .github/workflows)"` to prove #343 did not change workflow files
- [x] T017b Run `test -z "$(git diff --name-only origin/main...HEAD -- ':!docs/ci/ci-baseline-2026-05-15.md' ':!specs/003-ci-baseline-measurement/**')"` to prove #343 did not change source code or non-scope files
- [x] T018 Post and verify #333/#343 comments linking `docs/ci/ci-baseline-2026-05-15.md` after commit/push: #333 comment `4452104657`, #343 comment `4452106073`

## Dependencies

- T001-T002 before T011-T014.
- T003-T010 before T011-T014.
- T011-T014 before T015-T018.
- T018 requires pushed branch or PR URL.

## MVP

Complete #343 with one linked baseline artifact and no workflow behavior changes.
