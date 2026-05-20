# Tasks: NT-Matched Maker Order Scope

**Input**: Design documents from `specs/022-nt-maker-order-scope/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/maker-order-config.md`, `quickstart.md`

**Tests**: Required. TDD red/green evidence must be recorded before production behavior changes.

## Phase 1: Setup And Evidence Freeze

**Purpose**: Stop ad hoc work and make current state inspectable.

- [x] T001 Inventory current branch, dirty files, pinned NT rev, and no-mistakes daemon state in `specs/022-nt-maker-order-scope/research.md`
- [x] T002 Decide whether current provisional code is kept, reworked, or removed based only on task evidence in `specs/022-nt-maker-order-scope/tasks.md`
- [x] T003 Record exact PR #388 state and explain why it is trade-readiness context, not maker-order implementation proof, in `specs/022-nt-maker-order-scope/research.md`

T002 disposition: commit `97cbf828423578e09a604bf31bdaa91ec3573df3` is kept only as candidate/reference implementation. Completion requires clean replay proof under T014-T021 unless the user explicitly waives FR-006.

## Phase 2: End-To-End Investigation (Blocking)

**Purpose**: Prove the real path before implementation.

- [x] T004 [P] [US1] Map pinned NT Polymarket TIF mapping, post-only validation, GTD expiry, and HTTP `postOnly` serialization with file paths in `specs/022-nt-maker-order-scope/research.md`
- [x] T005 [P] [US1] Map bolt TOML parse, archetype validation, raw runtime mapping, strategy order construction, and `self.submit_order` path with file paths in `specs/022-nt-maker-order-scope/research.md`
- [x] T006 [US1] Identify exact coverage gap between bolt tests and NT adapter HTTP payload proof in `specs/022-nt-maker-order-scope/research.md`

## Phase 3: Pre-Implementation Adversarial Gate

**Purpose**: No implementation until review consensus or explicit block record.

- [x] T007 [P] [US3] Run internal adversarial review over investigation conclusions and record verdict in `specs/022-nt-maker-order-scope/research.md`
- [x] T008 [P] [US3] Run Claude review over investigation conclusions and record verdict or block reason in `specs/022-nt-maker-order-scope/research.md`
- [x] T009 [P] [US3] Run Gemini review over investigation conclusions and record verdict or block reason in `specs/022-nt-maker-order-scope/research.md`
- [x] T010 [P] [US3] Run Kimi review over investigation conclusions and record verdict or block reason in `specs/022-nt-maker-order-scope/research.md`
- [x] T011 [P] [US3] Run DeepSeek review over investigation conclusions and record verdict or block reason in `specs/022-nt-maker-order-scope/research.md`
- [x] T012 [P] [US3] Run GLM review over investigation conclusions and record verdict or block reason in `specs/022-nt-maker-order-scope/research.md`
- [x] T013 [US3] Resolve or document every pre-implementation finding before editing production code in `specs/022-nt-maker-order-scope/tasks.md`

## Phase 4: TDD Replay And Implementation

**Purpose**: Implement only approved scope, or accept the existing candidate only after clean replay proof.

- [x] T014 [US2] Create clean replay worktree from `origin/main` and record base SHA in `specs/022-nt-maker-order-scope/research.md`
- [x] T015 [P] [US2] Apply or write config-validation tests for approved maker entry/exit combinations in `tests/config_parsing.rs` and record red result before production diff
- [x] T016 [P] [US2] Apply or write runtime-mapping tests for approved maker entry/exit combinations in `tests/bolt_v3_strategy_registration.rs` and record base-green proof when no production diff is required
- [x] T017 [US2] Apply or write strategy order-object tests for approved maker entry/exit combinations in `src/strategies/binary_oracle_edge_taker.rs` and record red result before production diff
- [x] T018 [US2] Apply minimal archetype validation changes in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- [x] T019 [US2] Apply minimal strategy order-construction changes in `src/strategies/binary_oracle_edge_taker.rs`
- [x] T020 [US2] Update schema docs in `docs/bolt-v3/2026-04-25-bolt-v3-schema.md`
- [x] T021 [US2] Run focused green checks from `specs/022-nt-maker-order-scope/quickstart.md`

## Phase 5: Cleanup And Audit

**Purpose**: Remove slop and prove no overbuild.

- [x] T022 [US3] Run ai-slop-cleaner pass scoped to changed files and remove needless abstraction, duplication, or speculative code
- [x] T023 [US3] Run full verification from `specs/022-nt-maker-order-scope/quickstart.md`
- [ ] T024 [P] [US3] Run post-implementation Claude audit after exact PR head CI is green and record verdict or block reason
- [ ] T025 [P] [US3] Run post-implementation Gemini audit after exact PR head CI is green and record verdict or block reason
- [ ] T026 [P] [US3] Run post-implementation Kimi audit after exact PR head CI is green and record verdict or block reason
- [ ] T027 [P] [US3] Run post-implementation DeepSeek audit after exact PR head CI is green and record verdict or block reason
- [ ] T028 [P] [US3] Run post-implementation GLM audit after exact PR head CI is green and record verdict or block reason
- [ ] T029 [US3] Resolve or document every audit finding before merge/readiness completion

## Phase 6: Commit And Push

**Purpose**: Publish only after gates pass.

- [x] T030 Ensure `git status --short` contains only intended files
- [x] T031 Commit with a scope-accurate message
- [x] T032 Push `codex/maker-order-proof-clean`

## Phase 7: PR CI Regression Repair

**Purpose**: Keep exact PR-head CI green before any external audit gate.

- [x] T033 Inspect exact PR #434 CI logs and identify the failing check and test
- [x] T034 Replace the runtime NT source checkout read in `tests/config_parsing.rs` with compile-time embedded pinned NT source evidence
- [x] T035 Re-run the focused failed test, `cargo fmt -- --check`, `git diff --check`, `just source-fence`, and full `cargo test`
- [x] T036 Commit and push the CI regression fix before resuming external audits

T033 evidence: PR #434 head `769135106989e521cbc5e507e67442b6a376b74e` failed `nextest shard 4 of 4` because `polymarket_post_order_params_declares_camel_case_is_post_only_flag` tried to read the pinned NT query source from a Cargo git checkout at test runtime, but shard runners execute a nextest archive without that source checkout.

## Dependencies & Execution Order

- Phase 1 blocks all later phases.
- Phase 2 blocks implementation.
- Phase 3 blocks implementation.
- Phase 4 must follow TDD red before green.
- T022-T023 block commit/push.
- T024-T029 are post-push audit gates and remain blocked until exact PR head CI is green, per the repository review bar.
- Phase 6 happens after T023 so external reviewers can review an exact pushed head.
- Phase 7 blocks T024-T029 whenever PR CI regresses after push.

## Parallel Opportunities

- T004 and T005 can be reviewed independently.
- T008-T012 can run in parallel if external review tools are available.
- T015 and T016 can be written independently before shared implementation.
- T024-T028 can run in parallel after implementation verification.

## Implementation Strategy

1. Finish blocking investigation tasks.
2. Run pre-implementation adversarial quorum.
3. Replay or implement only task-approved maker scope with red/green evidence.
4. Verify, commit, and push the exact clean head.
5. Run post-push audit only after the exact PR head CI is green.
