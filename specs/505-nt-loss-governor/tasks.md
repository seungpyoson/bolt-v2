# Tasks: NT-First Loss Governor

**Input**: Design documents from `/specs/505-nt-loss-governor/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/loss-governor.md`
**Tests**: Required by spec; use red-green-refactor, one behavior at a time.

## Phase 1: Setup And Evidence

**Purpose**: Anchor the slice to current source truth and issue scope.

- [X] T001 Record current Cargo NT pin and pinned-source audit in `specs/505-nt-loss-governor/research.md`
- [X] T002 File GitHub issue #505 for the loss governor slice
- [X] T003 Run `.specify/scripts/bash/setup-tasks.sh --json` and generate `specs/505-nt-loss-governor/tasks.md`
- [X] T004 Confirm strategy implementation remains out of scope for this slice

---

## Phase 2: Foundational Policy Types

**Purpose**: Introduce the shared pure module with no runtime defaults and no integration wiring.

- [X] T005 [US1] Add failing `per_trade_loss_breach_rejects_admission` test in `src/bolt_v3_loss_governor.rs`
- [X] T006 [US1] Implement minimal `LossGovernorPolicy`, `LossSnapshot`, `LossAdmissionDecision`, `LossHaltReason`, and `evaluate_loss_admission` in `src/bolt_v3_loss_governor.rs`
- [X] T007 [US1] Export `bolt_v3_loss_governor` from `src/lib.rs`
- [X] T008 [US1] Run `cargo test --locked --lib bolt_v3_loss_governor::tests::per_trade_loss_breach_rejects_admission`

**Checkpoint**: Per-trade loss breach rejects through public module API.

---

## Phase 3: User Story 1 - Fresh Loss Snapshot Admission (Priority: P1)

**Goal**: Reject stale/missing/unattributed snapshots and configured per-trade or daily loss breaches.

**Independent Test**: Focused module tests evaluate synthetic NT-derived snapshots and assert deterministic halt reasons.

### Tests For User Story 1

- [X] T009 [US1] Add failing `daily_loss_breach_rejects_admission` test in `src/bolt_v3_loss_governor.rs`
- [X] T010 [US1] Implement daily loss threshold evaluation in `src/bolt_v3_loss_governor.rs`
- [X] T011 [US1] Run `cargo test --locked --lib bolt_v3_loss_governor::tests::daily_loss_breach_rejects_admission`
- [X] T012 [US1] Add failing `stale_missing_or_unattributed_snapshot_fails_closed` test in `src/bolt_v3_loss_governor.rs`
- [X] T013 [US1] Implement stale, missing, and unattributed snapshot fail-closed evaluation in `src/bolt_v3_loss_governor.rs`
- [X] T014 [US1] Run `cargo test --locked --lib bolt_v3_loss_governor::tests::stale_missing_or_unattributed_snapshot_fails_closed`
- [X] T015 [US1] Add passing acceptance test for fresh below-limit admission in `src/bolt_v3_loss_governor.rs`

**Checkpoint**: US1 passes without submit/live integration.

---

## Phase 4: User Story 2 - Rolling Loss And Drawdown Admission (Priority: P2)

**Goal**: Reject rolling-window loss and max drawdown breaches from NT-derived facts.

**Independent Test**: Focused module tests trip only the intended reasons.

### Tests For User Story 2

- [X] T016 [US2] Add failing `rolling_loss_breach_rejects_admission` test in `src/bolt_v3_loss_governor.rs`
- [X] T017 [US2] Implement rolling loss threshold evaluation in `src/bolt_v3_loss_governor.rs`
- [X] T018 [US2] Run `cargo test --locked --lib bolt_v3_loss_governor::tests::rolling_loss_breach_rejects_admission`
- [X] T019 [US2] Add failing `max_drawdown_breach_rejects_admission` test in `src/bolt_v3_loss_governor.rs`
- [X] T020 [US2] Implement max drawdown threshold evaluation in `src/bolt_v3_loss_governor.rs`
- [X] T021 [US2] Run `cargo test --locked --lib bolt_v3_loss_governor::tests::max_drawdown_breach_rejects_admission`
- [X] T022 [US2] Add deterministic multi-breach evidence ordering test in `src/bolt_v3_loss_governor.rs`

**Checkpoint**: US2 passes with deterministic evidence ordering.

---

## Phase 5: User Story 3 - Configured Submit/Live Integration (Priority: P3)

**Goal**: Bind configured loss-governor policy to live submit admission and feed it from NT-derived portfolio/position events.

**Independent Test**: Submit-admission/live-node tests prove missing/breached loss snapshots reject new risk before NT submit, fresh below-limit snapshots admit, risk-reducing exits remain possible under existing caps, and live builds carry policy into submit admission.

**PR #507 status**: Partially implemented in this branch. PR #507 includes configured submit-admission loss protection, live-node policy wiring, and a configured NT portfolio/position runtime feed that publishes loss snapshots from subscribed NT events. It still does not include positional-sizer live-path enforcement, cancel/flatten behavior, or NT `RiskEngine::set_trading_state` side effects.

- [X] T023 [US3] Add failing submit-admission test for configured loss governor rejecting new risk without a fresh snapshot
- [X] T024 [US3] Implement `BoltV3SubmitAdmissionState::new_unarmed_with_loss_governor`, `update_loss_snapshot`, deterministic loss halt evidence, and `admit_at`
- [X] T025 [US3] Add failing submit-admission test proving breached loss facts halt entries but allow risk-reducing exits within count cap
- [X] T026 [US3] Add `[risk.loss_governor]` TOML schema, config validation, fixture/example values, and parsing tests
- [X] T027 [US3] Wire configured policy into `build_live_node_with_clients` and assert live builds enable the shared submit-admission loss governor
- [X] T028 [US3] Add NT portfolio/position runtime feed that updates submit admission snapshots from subscribed NT events
- [X] T029 [US3] Add runtime-feed unit test for daily, rolling, per-trade, and drawdown facts derived from NT event types
- [X] T030 [US3] Update `specs/505-nt-loss-governor/research.md` with implementation evidence and live-protection boundary
- [X] T031 [US3] Require every enabled loss-governor threshold in config validation
- [X] T032 [US3] Bump decision-evidence schema to v6 for loss-governor halt evidence
- [X] T033 [US3] Keep mixed NT feed snapshots fresh only to the oldest contributing fact timestamp

---

## Phase 6: Verification And Cleanup

**Purpose**: Prove final diff and remove AI-slop smells.

- [X] T034 Run `cargo fmt --check`
- [X] T035 Run `cargo test --locked --lib`
- [X] T036 Run `cargo test --locked --test bolt_v3_submit_admission`
- [X] T037 Run `cargo test --locked --test config_parsing`
- [X] T038 Run `cargo test --locked --test bolt_v3_decision_evidence`
- [X] T039 Run `git diff --check`
- [X] T040 Run `just source-fence`
- [X] T041 Review changed files for scope drift and overclaim language
- [X] T042 Re-run `cargo test --locked --test bolt_v3_loss_runtime_feed`
- [X] T043 Re-run `cargo test --locked --test bolt_v3_submit_admission`
- [X] T044 Re-run `cargo fmt --check`, `git diff --check`, and `just source-fence`
- [X] T045 Run `cargo clippy --locked --lib -- -D warnings`
- [X] T046 Run `cargo test --locked --lib`

## Dependencies & Execution Order

- Phase 1 blocks all implementation.
- Phase 2 blocks US1 and US2.
- US1 and US2 are independent after Phase 2, but execute sequentially to preserve TDD evidence.
- US3 depends on the shared evaluator and config-free admission API.
- Verification and cleanup run last.

## MVP First

MVP is Phases 1-3: pure module rejects per-trade, daily, and stale snapshot failures. Phase 4 adds rolling and drawdown limits. Phase 5 turns it into submit-admission protection without adding cancel/flatten side effects.
