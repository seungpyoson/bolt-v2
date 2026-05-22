# Tasks: Developer-Tool Storage Hygiene

**Input**: Design documents from `/specs/024-developer-tool-storage-hygiene/`
**Prerequisites**: `plan.md`, `spec.md`, `evidence.md`, `research.md`, `data-model.md`, `contracts/developer-tool-storage-hygiene.md`, `quickstart.md`
**Tests**: Required by the user and constitution TDD gate. Each implementation task must follow RED/GREEN before cleanup.
**Execution Mode**: Sequential only. Do not start #454 or parallel branch work during any #375 task.

## Phase 1: Setup

**Purpose**: Make #375 the active Speckit package and verify branch/base hygiene.

- [ ] T001 Verify `.specify/feature.json`, `AGENTS.md`, and `CLAUDE.md` point to `specs/024-developer-tool-storage-hygiene/plan.md`.
- [ ] T002 Verify branch `codex/375-developer-tool-storage-hygiene` is based on current `origin/main` and record the SHA in `specs/024-developer-tool-storage-hygiene/evidence.md`.
- [ ] T003 Run placeholder and non-ASCII checks over `specs/024-developer-tool-storage-hygiene/` before external review.

---

## Phase 2: Foundational Gates

**Purpose**: Complete all planning and review gates before implementation.

- [ ] T004 Validate Phase 1 developer-tool enumeration coverage in `specs/024-developer-tool-storage-hygiene/evidence.md` against issue #375's required dimensions.
- [ ] T005 Generate and validate this task list in `specs/024-developer-tool-storage-hygiene/tasks.md`.
- [ ] T006 Request Claude plan/spec/tasks adversarial review for `specs/024-developer-tool-storage-hygiene/`.
- [ ] T007 Request Gemini plan/spec/tasks adversarial review for `specs/024-developer-tool-storage-hygiene/`.
- [ ] T008 Request GLM plan/spec/tasks adversarial review for `specs/024-developer-tool-storage-hygiene/`.
- [ ] T009 Request DeepSeek plan/spec/tasks adversarial review for `specs/024-developer-tool-storage-hygiene/`.
- [ ] T010 Record model, head SHA, scope, verdict, blockers, and skipped reviewers in `specs/024-developer-tool-storage-hygiene/evidence.md`.
- [ ] T011 Resolve or explicitly classify every source-proven planning blocker in `specs/024-developer-tool-storage-hygiene/`.
- [ ] T012 If implementation requires an operator-facing cleanup command, obtain explicit operator approval before editing `scripts/developer_tool_storage_hygiene.py`.

**Checkpoint**: No implementation starts until T006 through T012 are complete.

---

## Phase 3: User Story 1 - Enumerate Developer-Tool Writers (Priority: P1)

**Goal**: The repository contains a source-backed #375 enumeration reviewed for gaps and overlaps.

**Independent Test**: A reviewer can trace every listed developer-tool category to exact path family, growth shape, native retention support, and owner classification in `evidence.md`.

### Tests for User Story 1

- [ ] T013 [US1] Add a source-shape self-check in `scripts/test_developer_tool_storage_hygiene.py` that fails until required enumeration owner classes are represented by policy fixtures.

### Implementation for User Story 1

- [ ] T014 [US1] Add `ci/developer-tool-storage-hygiene.toml` with explicit surface IDs for Codex logs, Codex sessions, Codex sqlite report-only files, Factory log, rustup toolchains, and adjacent report-only classes.
- [ ] T015 [US1] Add the minimal policy loader and surface model in `scripts/developer_tool_storage_hygiene.py` to satisfy T013.
- [ ] T016 [US1] Update `docs/ops/developer-tool-storage-hygiene.md` with the #375 ownership map and native Codex/rustup capability evidence.
- [ ] T017 [US1] Re-run `python3 scripts/test_developer_tool_storage_hygiene.py` and record the green result in `specs/024-developer-tool-storage-hygiene/evidence.md`.

**Checkpoint**: User Story 1 is complete when the enumeration is machine-checked against the policy source.

---

## Phase 4: User Story 2 - Define Deterministic Cleanup Policy (Priority: P1)

**Goal**: #375-owned cleanup candidates are deterministic, dry-run first, and protect unsafe surfaces.

**Independent Test**: Synthetic Codex, Factory, and rustup fixtures classify only configured cleanup candidates and never classify protected/report-only paths for deletion.

### Tests for User Story 2

- [ ] T018 [US2] Add RED test in `scripts/test_developer_tool_storage_hygiene.py` for oversized Codex and Factory log rotation candidates.
- [ ] T019 [US2] Add RED test in `scripts/test_developer_tool_storage_hygiene.py` for stale Codex session TTL prune candidates.
- [ ] T020 [US2] Add RED test in `scripts/test_developer_tool_storage_hygiene.py` proving Codex sqlite db/WAL files are report-only.
- [ ] T021 [US2] Add RED test in `scripts/test_developer_tool_storage_hygiene.py` for active, default, and project-pinned rustup toolchain protection.
- [ ] T022 [US2] Add RED test in `scripts/test_developer_tool_storage_hygiene.py` for malformed or incomplete policy fail-closed validation.

### Implementation for User Story 2

- [ ] T023 [US2] Implement log rotation candidate classification in `scripts/developer_tool_storage_hygiene.py`.
- [ ] T024 [US2] Implement Codex session TTL candidate classification in `scripts/developer_tool_storage_hygiene.py`.
- [ ] T025 [US2] Implement report-only surface handling in `scripts/developer_tool_storage_hygiene.py`.
- [ ] T026 [US2] Implement rustup toolchain retention classification in `scripts/developer_tool_storage_hygiene.py`.
- [ ] T027 [US2] Implement malformed or incomplete policy fail-closed validation in `scripts/developer_tool_storage_hygiene.py`.
- [ ] T028 [US2] Re-run `python3 scripts/test_developer_tool_storage_hygiene.py` after each RED/GREEN slice and record final green result in `specs/024-developer-tool-storage-hygiene/evidence.md`.

**Checkpoint**: User Story 2 is complete when dry-run classification is deterministic over scratch fixtures.

---

## Phase 5: User Story 3 - Preflight Before Heavy Work (Priority: P2)

**Goal**: The operator can get read-only #375 storage pressure status before expensive local work.

**Independent Test**: Synthetic measurements over configured thresholds produce warning/error status without deleting files.

### Tests for User Story 3

- [ ] T029 [US3] Add RED preflight warning/error threshold test in `scripts/test_developer_tool_storage_hygiene.py`.
- [ ] T030 [US3] Add RED test in `scripts/test_developer_tool_storage_hygiene.py` proving out-of-repo browser and package-manager caches are reported but not owned.

### Implementation for User Story 3

- [ ] T031 [US3] Implement read-only preflight report construction in `scripts/developer_tool_storage_hygiene.py`.
- [ ] T032 [US3] Update `docs/ops/developer-tool-storage-hygiene.md` with preflight interpretation and out-of-scope handling.
- [ ] T033 [US3] Re-run `python3 scripts/test_developer_tool_storage_hygiene.py` and record final green result in `specs/024-developer-tool-storage-hygiene/evidence.md`.

**Checkpoint**: User Story 3 is complete when preflight is read-only and fail-closed under configured thresholds.

---

## Phase 6: User Story 4 - Verify Policy Without Touching Real Home Data (Priority: P2)

**Goal**: Reviewers can validate apply-safety behavior against scratch fixtures only.

**Independent Test**: Scratch apply behavior mutates only configured cleanup candidates and preserves protected/report-only paths.

### Tests for User Story 4

- [ ] T034 [US4] Add RED scratch apply test for log rotation in `scripts/test_developer_tool_storage_hygiene.py`.
- [ ] T035 [US4] Add RED scratch apply test for session TTL pruning in `scripts/test_developer_tool_storage_hygiene.py`.
- [ ] T036 [US4] Add RED scratch apply test proving protected rustup toolchains remain untouched in `scripts/test_developer_tool_storage_hygiene.py`.
- [ ] T037 [US4] Add RED scratch apply test proving report-only Codex sqlite files remain untouched in `scripts/test_developer_tool_storage_hygiene.py`.

### Implementation for User Story 4

- [ ] T038 [US4] Implement apply behavior over scratch/configured roots in `scripts/developer_tool_storage_hygiene.py` only if T012 approval permits the command surface.
- [ ] T039 [US4] Update `docs/ops/developer-tool-storage-hygiene.md` with dry-run/apply safety contract and native macOS config guidance.
- [ ] T040 [US4] Re-run `python3 scripts/test_developer_tool_storage_hygiene.py` and record final green result in `specs/024-developer-tool-storage-hygiene/evidence.md`.

**Checkpoint**: User Story 4 is complete when apply behavior is proven safe on scratch fixtures or explicitly scoped out by operator decision.

---

## Final Phase: Verification, Cleanup, PR, And Review

**Purpose**: Make the #375 branch review-ready without merging and without starting #454.

- [ ] T041 Run `python3 -m py_compile scripts/developer_tool_storage_hygiene.py scripts/test_developer_tool_storage_hygiene.py`.
- [ ] T042 Run `git diff --check origin/main...HEAD`.
- [ ] T043 Run relevant full Rust verification or record source-backed N/A in `specs/024-developer-tool-storage-hygiene/evidence.md`.
- [ ] T044 Run source-fence/schema/runtime literal checks if touched and record results in `specs/024-developer-tool-storage-hygiene/evidence.md`.
- [ ] T045 Run `$ai-slop-cleaner` on changed files and record the cleanup report in `specs/024-developer-tool-storage-hygiene/evidence.md`.
- [ ] T046 Run unresolved-marker scan over `specs/024-developer-tool-storage-hygiene/`, `ci/`, `scripts/`, and `docs/ops/` after #375 edits.
- [ ] T047 Commit and push branch `codex/375-developer-tool-storage-hygiene`.
- [ ] T048 Open the #375 PR and include issue scope, exact head SHA, evidence map, Speckit paths, tests, review status, no-mistakes status, remaining risk, and stop-before-merge note in the PR body.
- [ ] T049 Run no-mistakes on the exact PR head and verify the no-mistakes head equals the PR head.
- [ ] T050 Confirm exact-head GitHub CI is green for the #375 PR.
- [ ] T051 Request exact-PR-head external adversarial review and record Claude, Gemini, GLM, DeepSeek, and any skipped reviewers in `specs/024-developer-tool-storage-hygiene/evidence.md`.
- [ ] T052 Stop for operator approval before merge and before any #454 branch or implementation work.

## Dependencies & Execution Order

- Phase 1 must complete before Phase 2.
- Phase 2 must complete before any implementation.
- User Story 1 must complete before User Stories 2 through 4.
- User Story 2 must complete before User Story 4 apply behavior.
- Final Phase must complete before #375 is considered review-ready.
- #454 remains blocked until T048 through T052 are complete and the operator has not objected.

## Parallel Opportunities

None for this run. The operator requested one issue at a time and no concurrent work.

## Implementation Strategy

1. Finish Phase 1 and Phase 2 planning/review gates.
2. Implement User Story 1 as the MVP evidence/policy foundation.
3. Add User Story 2 dry-run classification via vertical TDD slices.
4. Add User Story 3 read-only preflight.
5. Add User Story 4 scratch apply behavior only if operator approval permits the command surface.
6. Complete final verification and PR review gates, then stop before merge.
