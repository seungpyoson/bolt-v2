# Tasks: PR #331 Phase 9 Completion

**Input**: Design documents from `specs/021-bolt-v3-phase9-current-main-audit/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/audit-evidence.md`, P0-P6 review dispositions

**Tests**: TDD required for every semantic merge fix and every remaining packet remediation.

**Organization**: Tasks are ordered by merge gate, then packet closure. PR #392 work is downstream only.

## Phase 1: Merge Gate

**Purpose**: Make PR #331 reviewable again after `origin/main` advanced.

- [x] T001 Verify current branch, PR #331 head, `origin/main`, and merge base with `git rev-parse` and `git merge-base`.
- [x] T002 Resolve remaining PR #331 merge state in `Cargo.toml`, `Cargo.lock`, `src/`, `tests/`, `docs/`, `scripts/`, and `specs/`.
- [x] T003 Prove no unresolved merge paths remain with `git diff --name-only --diff-filter=U`.
- [x] T004 Prove no conflict-marker tokens remain with a repository-wide `rg` scan.
- [x] T005 Preserve downstream production-readiness artifacts in `specs/013-production-live-readiness/`, `docs/bolt-v3/2026-05-18-production-readiness-contract.md`, and `tests/bolt_v3_production_readiness_contract.rs`.

## Phase 2: P6 Re-Verification

**Purpose**: Prove live-canary gate linkage remains fail-closed after merge.

- [x] T006 [P] [P6] Run `cargo test --test bolt_v3_live_canary_gate -- --nocapture`.
- [x] T007 [P] [P6] Run `cargo test --test bolt_v3_no_submit_readiness -- --nocapture`.
- [x] T008 [P6] Run `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture`.
- [x] T009 [P6] If T006-T008 fail, trace production code and test fixture linkage before any patch in `src/bolt_v3_live_canary_gate.rs`, `src/bolt_v3_no_submit_readiness.rs`, and affected `tests/*.rs`.
- [x] T010 [P6] Ensure readiness fixtures satisfy current `approval_id_hash`, `executable_identity`, and `config_bundle_checksum` linkage instead of bypassing the gate.

## Phase 3: AI-Slop Cleanup Pass

**Purpose**: Remove merge residue only after behavior is locked.

- [x] T011 [P] Classify merge-owned edits in `specs/021-bolt-v3-phase9-current-main-audit/ai-slop-cleanup-report.md`.
- [x] T012 Delete dead conflict residue only where tests or source scans prove it is unused.
- [x] T013 Remove duplicate fixture/helper code only if targeted tests remain green.
- [x] T014 Clean naming/error drift only where current schema or tests require it.

## Phase 4: P7-P9 Packet Review

**Purpose**: Complete PR #331 review obligations beyond P6.

- [ ] T015 [P7] Reconstruct P7 packet scope from PR #331 packet matrix and current exact head.
- [ ] T016 [P7] Run required local checks for P7 touched surfaces.
- [ ] T017 [P7] Obtain adversarial review if P7 process requires it, then fix or disprove findings.
- [ ] T018 [P8] Reconstruct P8 packet scope from PR #331 packet matrix and current exact head.
- [ ] T019 [P8] Run required local checks for P8 touched surfaces.
- [ ] T020 [P8] Obtain adversarial review if P8 process requires it, then fix or disprove findings.
- [ ] T021 [P9] Reconstruct P9 packet scope from PR #331 packet matrix and current exact head.
- [ ] T022 [P9] Run required local checks for P9 touched surfaces.
- [ ] T023 [P9] Obtain adversarial review if P9 process requires it, then fix or disprove findings.

## Phase 5: Full Verification

**Purpose**: Prove PR #331 exact head before any ready claim.

- [x] T024 Run `just fmt-check`.
- [x] T025 Run `git diff --check`.
- [x] T026 Run `just test`.
- [x] T027 Run `just clippy`.
- [x] T028 Run relevant source-fence/verifier commands named by touched specs/docs.
- [ ] T029 Commit merge/fixes only after T001-T028 pass or any failure is explicitly reported.
- [ ] T030 Push PR #331 branch and verify GitHub checks on pushed head.

## Phase 6: PR #392 Boundary

**Purpose**: Keep downstream work out of PR #331.

- [ ] T031 Confirm PR #392 still states PR #331 must merge first.
- [ ] T032 Do not implement PR #392 scope inside PR #331.
- [ ] T033 Record final PR #331 handoff: head SHA, checks, packet status P0-P9, residual blockers, and PR #392 next step.

## Dependencies & Execution Order

- Phase 1 blocks all packet review.
- Phase 2 blocks P7-P9 because P6 is current blocking packet.
- Phase 3 waits for targeted tests, not before.
- P7, P8, and P9 stay ordered unless user explicitly changes packet order.
- Phase 5 waits for all packet blockers closed.
- Phase 6 waits for PR #331 exact-head verification.

## Implementation Strategy

Finish merge gate first, then close P6 with proof, then run P7-P9 packet review. PR #392 remains downstream until PR #331 lands.
