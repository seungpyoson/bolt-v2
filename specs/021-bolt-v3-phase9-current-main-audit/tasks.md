# Tasks: PR #331 Phase 9 Completion

**Input**: Design documents from `specs/021-bolt-v3-phase9-current-main-audit/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/audit-evidence.md`, P0-P8 closure evidence

**Tests**: TDD is required for semantic runtime fixes. Current P9 sync is documentation-only; verification is artifact scans, diff hygiene, CI, and external review.

**Organization**: Tasks are ordered by packet closure. PR #392 work is downstream only.

## Phase 1: Merge Gate

**Purpose**: Make PR #331 reviewable after `origin/main` advanced.

- [x] T001 Verify current branch, PR #331 head, `origin/main`, and merge base with source commands.
- [x] T002 Resolve PR #331 merge state in Rust, tests, docs, scripts, config, and specs.
- [x] T003 Prove no unresolved merge paths remain.
- [x] T004 Prove no conflict-marker tokens remain.
- [x] T005 Preserve downstream production-readiness artifacts needed by PR #392.

## Phase 2: P6 Re-Verification

**Purpose**: Prove live-canary gate linkage remains fail-closed after merge.

- [x] T006 [P6] Run `cargo test --test bolt_v3_live_canary_gate -- --nocapture`.
- [x] T007 [P6] Run `cargo test --test bolt_v3_no_submit_readiness -- --nocapture`.
- [x] T008 [P6] Run `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture`.
- [x] T009 [P6] Trace production linkage before any fixture/schema patch.
- [x] T010 [P6] Ensure readiness fixtures satisfy current linkage instead of bypassing the gate.

## Phase 3: AI-Slop Cleanup Pass

**Purpose**: Remove merge residue only after behavior is locked.

- [x] T011 Classify merge-owned edits in `ai-slop-cleanup-report.md`.
- [x] T012 Delete dead conflict residue only where tests or source scans prove it unused.
- [x] T013 Remove duplicate fixture/helper code only if targeted tests remain green.
- [x] T014 Clean naming/error drift only where current schema or tests require it.

## Phase 4: P7-P9 Packet Reconstruction And Local Gates

**Purpose**: Reconstruct packet evidence and run local gates before exact-head review.

- [x] T015 [P7] Reconstruct P7 packet scope from PR #331 packet matrix and current exact head.
- [x] T016 [P7] Run required local checks for P7 touched surfaces.
- [x] T017 [P7] Obtain adversarial review, then record or defer nonblockers.
- [x] T018 [P8] Reconstruct P8 packet scope from PR #331 packet matrix and current exact head.
- [x] T019 [P8] Run required local checks for P8 touched surfaces.
- [x] T020 [P8] Obtain adversarial review, then record or defer nonblockers.
- [x] T021 [P9] Reconstruct P9 packet scope from PR #331 packet matrix and current exact head.
- [x] T022 [P9] Run local artifact checks for P9 touched surfaces.

## Phase 5: Exact-Head Verification

**Purpose**: Prove PR #331 exact head before any ready claim.

- [x] T024 Run stale-reference scan over P9 artifacts.
- [x] T025 Run debt-marker scan over P9 artifacts.
- [x] T026 Run `git diff --check`.
- [ ] T027 Commit P9 artifact sync after T022 and T024-T026 pass.
- [ ] T028 Push PR #331 branch.
- [ ] T029 Verify GitHub checks on pushed exact head.

## Phase 6: P9 External Review

**Purpose**: Review only the pushed exact head after CI is green.

- [ ] T030 [P9] Obtain six-reviewer adversarial review on pushed exact head, then fix or disprove findings.
- [ ] T031 Record P9 closure evidence in PR #331 after six-reviewer gate has no unresolved blockers.

The committed task list is not the final evidence ledger for T027-T031. Flipping those boxes after execution would create a new unreviewed head. Final completion evidence belongs in the PR #331 closure comment.

## Phase 7: PR #392 Boundary

**Purpose**: Keep downstream work out of PR #331.

- [ ] T032 Confirm PR #392 still states or implies PR #331 must land first.
- [ ] T033 Do not implement PR #392 scope inside PR #331.
- [ ] T034 Record final PR #331 handoff: head SHA, checks, packet status P0-P9, residual blockers, and PR #392 next step.

## Dependencies & Execution Order

- Phase 1 blocks packet review.
- Phase 2 blocks P7-P9 because P6 was the active blocking packet.
- P7 and P8 are closed before P9.
- P9 external review waits for clean pushed exact head and green CI.
- PR #392 boundary review waits for PR #331 P9 source-review closure.

## Implementation Strategy

Close P9 artifact sync first, commit and push, wait for CI, run six-model review, adjudicate blockers, then record PR #392 as downstream.
