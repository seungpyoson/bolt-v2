# Tasks: #466 Disk-Governance Verifier Decomposition

## Phase 1: Setup And Evidence

- [x] T001 Create #466 feature directory and seed evidence ledger in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [x] T002 Create #466 specification and quality checklist in `specs/466-decompose-disk-governance-verifiers/spec.md` and `specs/466-decompose-disk-governance-verifiers/checklists/requirements.md`
- [x] T003 Create #466 plan, research, data model, contract, and quickstart docs in `specs/466-decompose-disk-governance-verifiers/`
- [x] T004 Point active Spec Kit feature context at #466 in `.specify/feature.json` and `AGENTS.md`
- [x] T005 Rerun `python3 scripts/test_rust_verification_cache_retention.py` serially and record the result in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [x] T006 Run `python3 -m scripts.test_command_understanding` and record import-mode baseline in `specs/466-decompose-disk-governance-verifiers/evidence.md`

## Phase 2: Foundational Review Gates

- [ ] T007 Create pre-implementation review packet covering `specs/466-decompose-disk-governance-verifiers/spec.md`, `plan.md`, `tasks.md`, `evidence.md`, `research.md`, `data-model.md`, `contracts/ledger-resolution.md`, and `quickstart.md`
- [ ] T008 Run Claude pre-implementation review and record verdict/findings in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T009 Run Gemini pre-implementation review and record verdict/findings in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T010 Run Grok pre-implementation review and record verdict/findings in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T011 Run GLM pre-implementation review with audit metadata and record verdict/findings in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T012 Run DeepSeek pre-implementation review with audit metadata and record verdict/findings in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T013 Run Kimi pre-implementation review and record verdict/findings in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T014 Resolve or operator-waive every pre-implementation review finding before editing implementation code in `scripts/`

## Phase 3: User Story 1 - Govern The Full #466 Scope Ledger (Priority: P1)

**Independent Test**: `rg -n` over `specs/466-decompose-disk-governance-verifiers/evidence.md` proves all eight #466 ledger items are present and no item is silently moved or marked complete without evidence.

- [ ] T015 [US1] Add exact issue/PR command output references for #466, #464, #465, #461, and #454 to `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T016 [US1] Add final-state transition rules for `open`, `resolved`, `blocked`, and `operator-moved` to `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [x] T017 [US1] Add unresolved-marker scan command and current result for #466 docs to `specs/466-decompose-disk-governance-verifiers/evidence.md`

## Phase 4: User Story 2 - Reduce Runtime/Static Drift Without New Semantics (Priority: P2)

**Independent Test**: Each helper family has a characterization or parity proof before any extraction, cleanup, or explicit keep-local decision is marked resolved.

- [ ] T018 [P] [US2] Add/verify command tokenization and line-boundary characterization coverage in `scripts/test_command_understanding.py`
- [ ] T019 [US2] Mark ledger item 1 resolved as keep-local or approved extraction in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T020 [P] [US2] Add/verify shell command substitution characterization coverage in `scripts/test_command_understanding.py`
- [ ] T021 [US2] Mark ledger item 2 resolved as keep-local or approved extraction in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T022 [P] [US2] Add/verify renamed cargo/rustc characterization coverage in `scripts/test_command_understanding.py`, `scripts/test_rust_verification_cache_retention.py`, and `scripts/test_verify_ci_workflow_hygiene.py`
- [ ] T023 [US2] Mark ledger item 3 resolved as keep-local or approved extraction in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T024 [P] [US2] Add/verify wrapper handling characterization coverage in `scripts/test_command_understanding.py`, `scripts/test_rust_verification_cache_retention.py`, and `scripts/test_verify_ci_workflow_hygiene.py`
- [ ] T025 [US2] Mark ledger item 4 resolved as keep-local or approved extraction in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T026 [P] [US2] Add/verify full target-routing policy characterization coverage in `scripts/test_command_understanding.py`, `scripts/test_rust_verification_cache_retention.py`, and `scripts/test_verify_ci_workflow_hygiene.py`
- [ ] T027 [US2] Mark ledger item 5 resolved as keep-local or approved extraction in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T028 [US2] Audit behavior-preserving file split candidates and record split/no-split decision in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T029 [US2] If approved, mechanically split one concern boundary in `scripts/rust_verification.py`, `scripts/verify_ci_workflow_hygiene.py`, or their test files; otherwise mark ledger item 6 resolved as no-split with evidence in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T030 [US2] Add RED import-mode guard for any test-only import setup cleanup in `scripts/test_command_understanding.py`
- [ ] T031 [US2] Implement approved test-only import setup cleanup in `scripts/test_command_understanding.py`
- [ ] T032 [US2] Mark ledger item 7 resolved in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T033 [US2] Add/verify parity or identity guard for static `CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT` handling in `scripts/test_command_understanding.py`
- [ ] T034 [US2] Import shared `CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT` in `scripts/verify_ci_workflow_hygiene.py` if review approves; otherwise retain explicit parity guard/comment
- [ ] T035 [US2] Mark ledger item 8 resolved in `specs/466-decompose-disk-governance-verifiers/evidence.md`

## Phase 5: User Story 3 - Gate PRs And Issue Closure With Evidence (Priority: P3)

**Independent Test**: PR and final completion evidence map every claim to ledger rows, verification, exact-head CI, external reviews, and operator approval.

- [ ] T036 [US3] Run focused local verification for touched slice and record commands/results in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T037 [US3] Run `python3 -m py_compile` for touched Python files and record result in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T038 [US3] Run `git diff --check` and record result in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T039 [US3] Run `just ci-lint-workflow` when verifier/CI hygiene paths are touched and record result in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T040 [US3] Open a bounded PR for the completed slice and include #466 ledger coverage, non-goals, behavior preservation, tests, reviews, residual risk, and #466-open statement in the PR body
- [ ] T041 [US3] Confirm exact-head GitHub CI green for the PR head and record check/run IDs in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T042 [US3] Run post-implementation external reviews for Claude, Gemini, Grok, GLM, DeepSeek, and Kimi on the exact PR head and record verdicts/findings in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T043 [US3] Address all PR review comments and external findings before asking operator merge approval
- [ ] T044 [US3] After operator approval, merge with a normal merge commit unless the operator explicitly requests squash, then return to current `main` and continue unresolved #466 ledger items

## Final Phase: Whole-#466 Completion

- [ ] T045 Verify every ledger item final state is `resolved` or `operator-moved` in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T046 Run final whole-#466 local verification and record results in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T047 Run final whole-#466 external review across all merged #466 PRs and record reviewer verdicts in `specs/466-decompose-disk-governance-verifiers/evidence.md`
- [ ] T048 Update issue #466 with completion evidence only after all final checks pass
- [ ] T049 Ask operator approval to close #466; do not close without explicit approval

## Dependencies & Execution Order

1. Phase 1 setup and baseline evidence must complete before review.
2. Phase 2 pre-implementation external review must approve or be operator-waived before any implementation code changes.
3. Phase 3 ledger governance must remain current throughout every PR slice.
4. Phase 4 implementation tasks execute one helper family or mechanical cleanup at a time.
5. Phase 5 runs per PR slice before merge.
6. Final phase runs only after all ledger rows are resolved.

## Parallel Opportunities

- T008 through T013 are independent reviewer slots once the packet is prepared.
- T018, T020, T022, T024, and T026 are characterization audits, but implementation must still proceed one selected slice at a time after review.
- T036 through T039 can run in parallel where commands do not mutate shared state.

## Implementation Strategy

MVP is a reviewed plan/tasks/evidence packet with the full #466 ledger. First implementation candidate is the lowest-risk cleanup approved by review, likely static `CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT` drift cleanup or test-only import setup cleanup. Broader helper extractions require fresh characterization evidence and must not change accepted disk-governance semantics.
