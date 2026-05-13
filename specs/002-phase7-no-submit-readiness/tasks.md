# Tasks: Phase 7 No-submit Readiness

**Input**: Design documents from `/specs/002-phase7-no-submit-readiness/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/no-submit-readiness.md, quickstart.md

**Tests**: Required. User requested TDD: one behavior test, minimal implementation, refactor while green.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel when files do not conflict.
- **[Story]**: Maps to spec user story.
- Every implementation task has exact file paths.

## Phase 1: Setup And Review Gate

**Purpose**: Lock fresh-main planning and obtain external plan approval before runtime code.

- [x] T001 Record fresh-main evidence and stale PR audit summary in `specs/002-phase7-no-submit-readiness/research.md`.
- [x] T002 Run `no-mistakes daemon status` and record availability in handoff.
- [x] T003 Run Claude, DeepSeek, and GLM external review on `specs/002-phase7-no-submit-readiness/` plan artifacts.
- [x] T004 Triage all review findings into accepted fixes, evidence-backed disprovals, or explicit non-blocking deferrals in `specs/002-phase7-no-submit-readiness/external-review-phase7-disposition.md`.
- [x] T005 Stop before implementation unless Claude, DeepSeek, and GLM all approve or the user explicitly overrides a non-blocking disagreement.

---

## Phase 2: Foundational Report Contract

**Purpose**: Shared report schema and gate compatibility.

- [x] T006 [P] [US1] Write failing schema compatibility test in `tests/bolt_v3_no_submit_readiness.rs` proving producer report JSON is accepted by `check_bolt_v3_live_canary_gate`.
- [x] T007 [P] [US1] Write failing source-fence test in `tests/bolt_v3_no_submit_readiness.rs` proving `src/bolt_v3_no_submit_readiness.rs` and `tests/bolt_v3_no_submit_readiness_operator.rs` contain no submit, cancel, replace, amend, subscribe, or runner-loop tokens.
- [x] T008 [US1] Add shared no-submit report schema constants in `src/bolt_v3_no_submit_readiness_schema.rs`.
- [x] T009 [US1] Update `src/bolt_v3_live_canary_gate.rs` to consume shared schema constants without changing existing fail-closed behavior.
- [x] T010 [US1] Export the schema module from `src/lib.rs`.
- [x] T011 [US1] Run targeted schema/gate tests and capture red/green evidence.

---

## Phase 3: User Story 1 - Produce Local No-submit Readiness Evidence (Priority: P1)

**Goal**: Local no-submit readiness report is produced through current main boundaries and accepted by live-canary gate.

**Independent Test**: `cargo test --test bolt_v3_no_submit_readiness -- --nocapture`.

- [x] T012 [P] [US1] Write failing local runner test in `tests/bolt_v3_no_submit_readiness.rs` for satisfied controlled-connect and controlled-disconnect stages.
- [x] T013 [P] [US1] Write failing redaction test in `tests/bolt_v3_no_submit_readiness.rs` proving resolved secret values do not appear in debug or JSON output.
- [x] T014 [P] [US1] Write failing connect-failure, reference-readiness failure, byte-cap, and double-failure cleanup tests in `tests/bolt_v3_no_submit_readiness.rs`.
- [x] T015 [US1] Add `src/bolt_v3_no_submit_readiness.rs` report model, redaction model, and local sequencing API.
- [x] T016 [US1] Add current-main-safe controlled-connect/disconnect runner support in `src/bolt_v3_live_node.rs` without exposing broad `node_mut`.
- [x] T017 [US1] Export `bolt_v3_no_submit_readiness` from `src/lib.rs`.
- [x] T018 [US1] Run `cargo test --test bolt_v3_no_submit_readiness -- --nocapture` and capture green output.
- [x] T019 [US1] Run `cargo test --test bolt_v3_live_canary_gate -- --nocapture` and capture green output.
- [x] T020 [US1] Run focused clippy on new Phase 7 files when the local test slice is green.

---

## Phase 3B: Implementation Discovery Correction - Reference Readiness Through NT Cache

**Goal**: Replace connect-success reference readiness with an NT-owned cache proof after controlled start.

**Independent Test**: `cargo test --test bolt_v3_no_submit_readiness -- --nocapture` proves missing required reference instruments fail closed and all required cache entries satisfy `reference_readiness`.

- [x] T040 [US1] Record revised NT start/stop/cache-readiness design in `specs/002-phase7-no-submit-readiness/plan.md`, `research.md`, `contracts/no-submit-readiness.md`, `quickstart.md`, and `external-review-phase7-disposition.md`.
- [x] T041 [US1] Run Claude, DeepSeek, and GLM external review on revised Phase 7 plan at clean pushed head before runtime code changes.
- [x] T042 [US1] Triage revised-plan review findings into accepted fixes, evidence-backed disprovals, or explicit user-approved deferrals.
- [x] T043 [P] [US1] Write failing behavior test in `tests/bolt_v3_no_submit_readiness.rs` proving required strategy `reference_data` instruments missing from NT cache fail `reference_readiness` and still record controlled stop.
- [x] T044 [P] [US1] Write failing behavior test in `tests/bolt_v3_no_submit_readiness.rs` proving all required strategy `reference_data` instruments present in NT cache before timeout satisfy `reference_readiness` and record controlled stop.
- [x] T045 [US1] Implement bounded NT `LiveNode::start()` / readiness / `LiveNode::stop()` helper in `src/bolt_v3_live_node.rs` without `run()` and without broad `node_mut`; ensure stop is called and reported after start success, reference-cache failure, and partial startup failure.
- [x] T046 [US1] Implement `reference_readiness` over required strategy `reference_data` instruments using NT cache evidence only with bounded recheck from existing live-node timeout config and no new hardcoded poll interval.
- [x] T047 [US1] Update `tests/bolt_v3_no_submit_readiness_operator.rs` so the ignored real harness only expects gate acceptance after NT cache reference proof.
- [x] T048 [US1] Re-run targeted tests, focused clippy, source fences, strategy `on_start`/submit-admission audit, `cargo fmt --check`, `git diff --check`, and no-mistakes after implementation correction.

---

## Phase 4: User Story 2 - Gate Real No-submit Readiness Behind Explicit Operator Approval (Priority: P2)

**Goal**: Real SSM/venue readiness harness exists but is ignored by default and approval-gated before any side effect.

**Independent Test**: `cargo test --test bolt_v3_no_submit_readiness_operator -- --nocapture` shows ignored by default.

- [x] T021 [P] [US2] Write failing test in `tests/bolt_v3_no_submit_readiness.rs` proving missing approval id fails before secret resolution.
- [x] T022 [P] [US2] Write failing test in `tests/bolt_v3_no_submit_readiness.rs` proving approval mismatch fails before secret resolution.
- [x] T023 [US2] Implement real-run approval validation in `src/bolt_v3_no_submit_readiness.rs`.
- [x] T024 [US2] Add ignored operator harness in `tests/bolt_v3_no_submit_readiness_operator.rs`.
- [x] T025 [US2] Run default operator-harness test and capture ignored-by-default output.
- [x] T026 [US2] Do not run ignored real SSM/venue command without explicit user approval in current thread.

---

## Phase 5: User Story 3 - Preserve Phase 8 Safety Boundary (Priority: P3)

**Goal**: Phase 7 artifacts do not imply Phase 8 live-order readiness.

**Independent Test**: Source/docs checks show Phase 8 remains blocked pending real report and strategy-input safety audit.

- [x] T027 [P] [US3] Add Phase 8 boundary assertions to `tests/bolt_v3_no_submit_readiness.rs`.
- [x] T028 [US3] Update `specs/002-phase7-no-submit-readiness/quickstart.md` only with explicit blocked-live wording and no executable live-capital command.
- [x] T029 [US3] Record Phase 8 blocked state in `specs/002-phase7-no-submit-readiness/external-review-phase7-disposition.md`.

---

## Phase 6: Verification And PR Readiness

**Purpose**: Verify Phase 7 branch before PR or implementation-complete claim.

- [x] T030 Run `cargo test --test bolt_v3_no_submit_readiness -- --nocapture`.
- [x] T031 Run `cargo test --test bolt_v3_no_submit_readiness_operator -- --nocapture`.
- [x] T032 Run `cargo test --test bolt_v3_live_canary_gate -- --nocapture`.
- [x] T033 Run relevant integration tests for live-node controlled connect if touched.
- [x] T034 Run `cargo fmt --check`.
- [x] T035 Run `git diff --check`.
- [x] T036 Run runtime literal/hardcode checks relevant to new files.
- [x] T037 Run no-mistakes status/checks if available.
- [ ] T038 Run full `cargo test` and clippy only when branch is locally green enough for PR readiness.
- [ ] T039 Keep worktree clean before requesting further external review or opening PR.

## Dependencies & Execution Order

- Phase 1 blocks implementation.
- Phase 2 blocks Phase 3.
- Phase 3 can complete local MVP without Phase 4 real operator approval.
- Phase 4 adds real-readiness harness but does not run it without approval.
- Phase 5 can run after Phase 3.
- Phase 6 runs after local implementation.

## Parallel Opportunities

- T006 and T007 can be written in parallel.
- T012, T013, and T014 can be written in parallel after Phase 2.
- T020 and T021 can be written in parallel.
- T026 and T027 can be handled in parallel after Phase 3.

## Implementation Strategy

1. Finish Phase 1 review gate.
2. Implement Phase 2 schema/gate compatibility.
3. Implement Phase 3 local no-submit MVP.
4. Add Phase 4 ignored operator harness but do not run real command.
5. Preserve Phase 8 blocked state.
6. Run verification before any PR or completion claim.
