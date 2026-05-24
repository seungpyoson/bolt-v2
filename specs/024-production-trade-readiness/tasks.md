# Tasks: Production Trade Readiness

**Input**: `specs/024-production-trade-readiness/spec.md`, `specs/024-production-trade-readiness/plan.md`, and `specs/024-production-trade-readiness/evidence.md`
**Branch/PR**: `goal/024-production-trade-readiness`, PR #480
**Policy**: one readiness PR; no order-intent-layer work; no #466 decomposition-ledger work.

## Phase 1: Baseline And Task-List Approval

**Purpose**: Lock scope and approve this task list before production code work resumes.

- [x] T001 Record current git, PR, issue, Speckit, readiness-ledger, code, and T038-branch evidence in `specs/024-production-trade-readiness/evidence.md`.
- [x] T002 Update PR #480 body to reference `specs/024-production-trade-readiness/` as the active readiness task list, branch `goal/024-production-trade-readiness`, and to keep PR #479/#466 out of scope.
- [x] T003 Remove #466 verifier-characterization/decomposition files from PR #480 and record the exact removed paths in `specs/024-production-trade-readiness/scope-resolution.md`.
- [x] T004 Send `specs/024-production-trade-readiness/{spec.md,plan.md,tasks.md,evidence.md,scope-resolution.md}` plus `AGENTS.md` policy context to Claude, Gemini, DeepSeek, GLM, Kimi, and Grok for task-list review only; record verdicts in `specs/024-production-trade-readiness/external-tasklist-review.md`.
- [x] T005 Resolve every blocking task-list review finding in `specs/024-production-trade-readiness/tasks.md` and record disposition in `specs/024-production-trade-readiness/external-tasklist-review.md`.

## Phase 2: T038 Branch And Existing Issue Hygiene

**Purpose**: Avoid blind ports and close/update issue state that current evidence already resolves.

- [x] T006 [P] Complete a targeted `t038-operator-config-snapshot` port audit in `specs/024-production-trade-readiness/t038-port-audit.md`, comparing each unique old branch behavior to current #480/main no-submit code and recording any exact missing patch.
- [x] T007 [P] Verify whether #409 PortfolioSnapshot acceptance criteria are satisfied by current source and tests; record close/update evidence in `specs/024-production-trade-readiness/issue-409-portfolio-snapshot.md`.
- [x] T008 [P] Update #385 evidence with the current distinction between historical T038 no-submit success and missing final-packet T131/T122 no-submit proof in `specs/024-production-trade-readiness/issue-385-no-submit.md`.

## Phase 3: User Story 2 - Real Decision Evidence

**Goal**: Close T124/T125 with real current-head runtime artifacts, not fixtures or static generation.

**Independent Test**: final-packet generation rejects missing real market-selection and strategy-input evidence, then accepts only current-head artifact paths and hashes bound by `[live_canary.operator_evidence]`.

- [ ] T009 [US2] Add a RED final-packet test in `tests/bolt_v3_operator_artifacts.rs` proving fixture/static market-selection evidence cannot satisfy T124.
- [ ] T010 [US2] Produce or wire the real current-head runtime market-selection artifact path in `src/bolt_v3_operator_artifacts.rs` without hardcoded venue, market, price, quantity, or timeout values.
- [ ] T011 [US2] Add a RED final-packet test in `tests/bolt_v3_operator_artifacts.rs` proving missing real runtime JSONL strategy-input chain cannot satisfy T125.
- [ ] T012 [US2] Produce or wire the real current-head runtime strategy-input evidence chain in `src/bolt_v3_operator_artifacts.rs` and `src/strategies/binary_oracle_edge_taker.rs`.
- [ ] T013 [US2] Bind T124/T125 artifact paths and hashes through `[live_canary.operator_evidence]` in `config/live.local.toml` and update runtime-literal audit rows in `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`.
- [ ] T014 [US2] Run focused T124/T125 tests plus runtime-literal verification and record the evidence in `specs/024-production-trade-readiness/evidence.md`.

## Phase 4: User Story 3 - Pre-Run State Collectors

**Goal**: Close T126 by replacing caller-supplied proof gaps with source-owned collectors.

**Independent Test**: pre-run proof generation fails for each missing collector and passes only when every required pre-run source proof is present.

- [x] T015 [P] [US3] Add RED tests for venue account, open-orders, and positions collectors in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T016 [US3] Implement venue account, open-orders, and positions source collectors in `src/bolt_v3_operator_artifacts.rs`.
- [x] T017 [P] [US3] Add RED tests for funding and margin source collectors in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T018 [US3] Implement funding and margin source collectors in `src/bolt_v3_operator_artifacts.rs`.
- [x] T019 [P] [US3] Add RED tests for approved egress identity source collector in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T020 [US3] Implement approved egress identity source collector in `src/bolt_v3_operator_artifacts.rs`.
- [x] T021 [P] [US3] Add RED tests for CLOB V2 adapter signing, collateral accounting, and fee behavior collectors in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T022 [US3] Implement CLOB V2 adapter signing, collateral accounting, and fee behavior collectors in `src/bolt_v3_operator_artifacts.rs`.
- [ ] T023 [US3] Bind all T126 collector outputs through operator evidence config in `config/live.local.toml` and runtime-literal audit rows in `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`.
- [ ] T024 [US3] Run focused T126 tests plus runtime-literal verification and record evidence in `specs/024-production-trade-readiness/evidence.md`.

## Phase 5: User Story 4 - Abort-Plan Collectors

**Goal**: Close T127 by replacing caller-supplied abort proof gaps with source-owned collectors.

**Independent Test**: abort-plan proof generation fails for each missing abort collector and passes only when every required abort proof is present.

- [x] T025 [P] [US4] Add RED tests for NT accepted and venue pending abort collectors in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T026 [US4] Implement NT accepted and venue pending abort collectors in `src/bolt_v3_operator_artifacts.rs`.
- [x] T027 [P] [US4] Add RED tests for partial-fill abort collector in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T028 [US4] Implement partial-fill abort collector in `src/bolt_v3_operator_artifacts.rs`.
- [ ] T029 [P] [US4] Add RED tests for network-partition abort collector in `tests/bolt_v3_operator_artifacts.rs`.
- [ ] T030 [US4] Implement network-partition abort collector in `src/bolt_v3_operator_artifacts.rs`.
- [ ] T031 [P] [US4] Add RED tests for panic-gate and service-policy collector in `tests/bolt_v3_operator_artifacts.rs`.
- [ ] T032 [US4] Implement panic-gate and service-policy collector in `src/bolt_v3_operator_artifacts.rs`.
- [ ] T033 [US4] Bind all T127 collector outputs through operator evidence config in `config/live.local.toml` and runtime-literal audit rows in `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`.
- [ ] T034 [US4] Run focused T127 tests plus runtime-literal verification and record evidence in `specs/024-production-trade-readiness/evidence.md`.

## Phase 6: User Story 5 - Final Packet

**Goal**: Close T128 with a blocker-free final packet that consumes T124-T127 real artifacts.

**Independent Test**: `operator-artifacts verify-final` passes only for the exact root TOML and final packet with matching artifact hashes.

- [ ] T035 [US5] Add a RED end-to-end final-packet test in `tests/bolt_v3_operator_artifacts.rs` that fails until T124-T127 artifacts and `[live_canary.operator_evidence]` exist together.
- [ ] T036 [US5] Assemble blocker-free `static-artifacts-manifest.json`, `approval-envelope.json`, and `operator-evidence-packet.json` from real artifacts and record paths in `specs/024-production-trade-readiness/final-packet.md`.
- [ ] T037 [US5] Update the approved root TOML operator-evidence block in `config/live.local.toml` with final artifact paths and hashes without printing secrets.
- [ ] T038 [US5] Run `operator-artifacts verify-final` against the exact root TOML and final packet; record command, head, hashes, and result in `specs/024-production-trade-readiness/evidence.md`.

## Phase 7: Exact-Head Verification And Final Review

**Goal**: Close T130 before any final-packet no-submit or tiny-capital canary operation.

**Independent Test**: local checks, GitHub CI, and external reviews all target the same pushed head.

- [ ] T039 [US5] Run focused readiness tests: `tests/bolt_v3_operator_artifacts.rs`, `tests/bolt_v3_tiny_canary_preconditions.rs`, `tests/bolt_v3_tiny_canary_operator.rs`, `tests/bolt_v3_live_canary_gate.rs`, and `tests/bolt_v3_cli.rs`.
- [ ] T040 [US5] Run full local verification: `cargo fmt --check`, `git diff --check`, runtime-literal verification, source/slop/hardcode/secret scans, and readiness test suites; record output summary in `specs/024-production-trade-readiness/evidence.md`.
- [ ] T041 [US5] Push PR #480 and record exact-head GitHub CI evidence in `specs/024-production-trade-readiness/evidence.md`.
- [ ] T042 [US5] Send exact-head final review to Claude, Gemini, DeepSeek, GLM, Kimi, and Grok; record unanimous approvals or explicit waivers in `specs/024-production-trade-readiness/external-final-review.md`.

## Phase 8: Approved Operations

**Goal**: Close T131/T122 and T116/T046 only after final packet and T130 pass.

**Independent Test**: final-packet no-submit passes before tiny-capital canary; both are bound to exact head, root TOML, final packet, and retained evidence hashes.

- [ ] T043 [US5] Execute T131/T122 final-packet EC2/EIP no-submit rerun with the verified root TOML and final operator packet; record evidence in `specs/024-production-trade-readiness/final-no-submit.md`.
- [ ] T044 [US5] Execute T116/T046 tiny-capital canary with the verified root TOML and final operator packet; record evidence in `specs/024-production-trade-readiness/tiny-canary.md`.
- [ ] T045 [US5] Run post-run artifact/log secret scan and record retention/purge decision in `specs/024-production-trade-readiness/post-run-hygiene.md`.
- [ ] T046 [US5] Update #369, #385, #409, #360, and PR #480 with exact final readiness status and record links in `specs/024-production-trade-readiness/readiness-ledger.md`.

## Dependencies

- T001-T005 must complete before implementation resumes.
- T006-T008 may run in parallel after T005.
- T009-T014, T015-T024, and T025-T034 can be parallelized by collector group after T005, but each implementation task depends on its RED test.
- T035-T038 require T009-T034.
- T039-T042 require T035-T038.
- T043 requires T042.
- T044 requires T043.
- T045-T046 require T044.

## MVP

The first implementation slice after task-list approval is T019-T020, approved egress identity source collector, unless external reviewers identify a different first blocker. It is non-trading, source-owned, and small enough for a clean RED/GREEN cycle.
