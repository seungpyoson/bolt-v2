# Tasks: Bolt-v3 Phase 9 Comprehensive Audit

**Input**: Design documents from `specs/019-bolt-v3-phase9-audit-fresh/`
**Prerequisites**: `spec.md`, `plan.md`, `research.md`, `data-model.md`, `contracts/audit-evidence.md`

**Tests**: Phase 9 implementation tasks use TDD. This planning slice has only artifact verification.

## Phase 1: Audit Artifacts

**Purpose**: Create reviewable Phase 9 planning and audit package.

- [x] T001 [P] Write Phase 9 spec in `specs/019-bolt-v3-phase9-audit-fresh/spec.md`.
- [x] T002 [P] Write requirements checklist in `specs/019-bolt-v3-phase9-audit-fresh/checklists/requirements.md`.
- [x] T003 [P] Write plan in `specs/019-bolt-v3-phase9-audit-fresh/plan.md`.
- [x] T004 [P] Write research findings in `specs/019-bolt-v3-phase9-audit-fresh/research.md`.
- [x] T005 [P] Write data model and evidence contract in `data-model.md` and `contracts/audit-evidence.md`.
- [x] T006 [P] Write preliminary audit report and cleanup report.
- [x] T007 [P] Write external review prompt.

## Phase 2: Local Verification

**Purpose**: Prove artifacts are reviewable before push or external review.

- [x] T008 Run debt-marker scan over Phase 9 artifacts.
- [x] T009 Run `git diff --check`.
- [x] T010 Record no-mistakes runtime proof.
- [ ] T011 Commit Phase 9 artifacts.

## Phase 3: User-Gated Push And External Review

**Purpose**: Request external review only after branch is clean, pushed, and exact-head checks are available.

- [ ] T012 Ask user approval to push `019-bolt-v3-phase9-audit-fresh`.
- [ ] T013 Push branch after approval.
- [ ] T014 Run exact-head checks available for the pushed branch.
- [ ] T015 Run Claude custom review against Phase 9 artifacts.
- [ ] T016 Run DeepSeek custom review after approval-token evidence.
- [ ] T017 Run GLM custom review after approval-token evidence.
- [ ] T018 Record findings and dispositions in `external-review-phase9-disposition.md`.

## Phase 4: Cleanup Implementation Gate

**Purpose**: Begin no cleanup until review and user approval permit it.

- [ ] T019 If reviewers approve and user approves implementation, choose one bounded cleanup candidate.
- [ ] T020 Write one failing public behavior test or source fence for that candidate.
- [ ] T021 Implement minimal cleanup.
- [ ] T022 Run targeted tests and source fences.
- [ ] T023 Re-run audit scans for changed scope.
- [ ] T024 Commit only the bounded cleanup slice.

## Stop Conditions

- External review has unresolved blocker.
- Branch is dirty or unpushed before review.
- User has not approved push or implementation.
- Cleanup scope lacks a behavior test or source fence.
- Any live-order, soak, or secret-exposing action is requested without exact approval.
