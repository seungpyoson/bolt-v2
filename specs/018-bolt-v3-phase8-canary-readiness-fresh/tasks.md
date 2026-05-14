# Tasks: Bolt-v3 Phase 8 Tiny-capital Canary Machinery

**Input**: Design documents from `specs/018-bolt-v3-phase8-canary-readiness-fresh/`
**Prerequisites**: Phase 8 spec, checklist, plan, research, stale PR audit, strategy-input safety audit, data model, and evidence contract.

## Phase 1: Planning Gate

**Purpose**: Lock scope and prevent stale-branch continuation.

- [x] T001 Record fresh-main anchor and baseline lib test in `research.md`.
- [x] T002 Audit PR #318/#319/#320 as reference-only in `stale-pr-audit.md`.
- [x] T003 Record strategy-input safety audit and live-action block in `strategy-input-safety-audit.md`.
- [x] T004 Create Phase 8 spec/checklist/plan/data-model/contracts/quickstart/tasks artifacts.
- [x] T005 Run placeholder/debt scan on Phase 8 spec artifacts.
- [x] T006 Run `git diff --check`.
- [x] T007 Commit Phase 8 planning artifacts.
- [ ] T008 Push planning branch only after explicit approval for GitHub mutation.
- [ ] T009 Request external plan review only after branch is pushed and review gate allows it.

## Phase 2: User Story 1 - Block Unsafe Canary Start (P1)

**Goal**: Local preflight blocks before build/runner/submit when Phase 7, gate, strategy audit, or approval input is missing.

**Independent Test**: `cargo test --test bolt_v3_tiny_canary_preconditions preflight_blocks_missing_phase7_report -- --nocapture`.

### Tests First

- [ ] T010 [US1] Write failing test in `tests/bolt_v3_tiny_canary_preconditions.rs` for missing Phase 7 no-submit report blocking before build.
- [ ] T011 [US1] Write failing test in `tests/bolt_v3_tiny_canary_preconditions.rs` for blocked strategy audit blocking before build.
- [ ] T012 [US1] Write failing test in `tests/bolt_v3_tiny_canary_preconditions.rs` proving non-positive realized volatility blocks before build.
- [ ] T013 [US1] Write failing test in `tests/bolt_v3_tiny_canary_preconditions.rs` proving non-positive time to expiry blocks before build.
- [ ] T014 [US1] Write failing source fence proving Phase 8 harness contains no direct `LiveNode::run`, manual submit, manual cancel, or Bolt reconciliation tokens.

### Minimal Implementation

- [ ] T015 [US1] Add `src/bolt_v3_tiny_canary_evidence.rs` with `Phase8CanaryPreflight` and redacted block reasons.
- [ ] T016 [US1] Export module from `src/lib.rs`.
- [ ] T017 [US1] Make T010-T014 green with no live network, no SSM calls, and no NT runner entry.
- [ ] T018 [US1] Run focused precondition tests and commit this vertical slice.

## Phase 3: User Story 2 - Produce Dry Canary Evidence (P2)

**Goal**: Produce redacted dry/no-submit evidence proving path shape without order placement.

**Independent Test**: `cargo test --test bolt_v3_tiny_canary_preconditions dry_canary_evidence_serializes_join_keys_without_secrets -- --nocapture`.

### Tests First

- [ ] T019 [US2] Write failing serialization test for `Phase8CanaryEvidence` required join keys and no raw secret fields.
- [ ] T020 [US2] Write failing test for decision evidence unavailable -> blocked before submit admission.
- [ ] T021 [US2] Write failing test for rejected live canary gate -> blocked before runner.

### Minimal Implementation

- [ ] T022 [US2] Add minimal `Phase8CanaryEvidence` data structures and writer.
- [ ] T023 [US2] Hash approval id and config/SSM identities; do not print raw secrets.
- [ ] T024 [US2] Make T019-T021 green with fixture-only data.
- [ ] T025 [US2] Run focused dry evidence tests and commit this vertical slice.

## Phase 4: User Story 3 - Prepare One-order Operator Harness (P3)

**Goal**: Provide an ignored operator harness skeleton that is inert by default and can only use production bolt-v3 path after exact approval.

**Independent Test**: `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture` reports one ignored test.

### Tests First

- [ ] T026 [US3] Write failing source test requiring `#[ignore]`, exact operator inputs, `build_bolt_v3_live_node`, and `run_bolt_v3_live_node` within a `tokio::task::LocalSet` context.
- [ ] T027 [US3] Write failing source test requiring one-order cap and forbidding loops/manual submit.
- [ ] T028 [US3] Write failing source test proving the one-order cap comes from the approved config fixture, not a harness literal.
- [ ] T029 [US3] Write failing source test forbidding direct exec-engine cancel and Bolt-owned reconciliation.

### Minimal Implementation

- [ ] T030 [US3] Add `tests/bolt_v3_tiny_canary_operator.rs` ignored harness skeleton.
- [ ] T031 [US3] Require exact head SHA, root TOML path, root TOML checksum, SSM manifest hash, approval id, and evidence path before build.
- [ ] T032 [US3] Assert the configured Phase 8 tiny-canary cap before runner; for this slice the approved config fixture value is `max_live_order_count == 1`.
- [ ] T033 [US3] Keep live order execution blocked unless user approval is supplied at runtime.
- [ ] T034 [US3] Run harness default test and source fences, then commit this vertical slice.

## Phase 5: Verification And Review Gate

**Purpose**: Verify local readiness before PR/review.

- [ ] T035 Run `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture`.
- [ ] T036 Run `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture`.
- [ ] T037 Run relevant existing tests: `cargo test --test bolt_v3_submit_admission -- --nocapture`, `cargo test --test bolt_v3_decision_evidence -- --nocapture`, `cargo test --test bolt_v3_live_canary_gate -- --nocapture`.
- [ ] T038 Run `cargo fmt --check`.
- [ ] T039 Run `git diff --check`.
- [ ] T040 Run runtime literal/hardcode/debt scans for changed files.
- [ ] T041 Run no-mistakes runtime proof if available: `which no-mistakes`, `no-mistakes --version`, `no-mistakes daemon status`.
- [ ] T042 Run full `cargo test` and `cargo clippy -- -D warnings` when branch is ready and runtime cost is acceptable.
- [ ] T043 Commit verification updates.
- [ ] T044 Present evidence and recommendation before any push.
- [ ] T045 Push only with explicit approval or after user-approved GitHub mutation gate.
- [ ] T046 Run `gh pr checks` after push.
- [ ] T047 Request external implementation review only after exact-head checks are green.

## Phase 6: Stopped Live Action Gate

**Purpose**: Make the stop explicit.

- [ ] T048 Do not run ignored live operator harness without user approval naming exact head SHA and exact command.
- [ ] T049 If user approves live action later, rerun strategy-input safety audit against the approved config first.
- [ ] T050 If any safety audit item remains blocked, do not proceed to live order.

## Dependencies

- Phase 1 must complete before any implementation.
- External plan review must approve before Phase 2 starts.
- Phase 2 must complete before Phase 3.
- Phase 3 must complete before Phase 4.
- Phase 4 must complete before verification and external implementation review.
- Live action remains outside implementation readiness unless explicitly approved.
