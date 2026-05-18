# Tasks: Thin Bolt-v3 Live Canary Path

**Input**: Design documents from `/specs/001-thin-live-canary-path/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/`

All code tasks use TDD. For each behavior: write failing test, run it and capture expected failure, implement minimal code, run green test, run phase verification, then commit. Do not batch unrelated slices.

## Phase 1: Planning And Evidence

**Purpose**: Lock constraints before runtime code.

- [x] T001 Record current-state evidence in `specs/001-thin-live-canary-path/research.md`.
- [x] T002 Replace `.specify/memory/constitution.md` with bolt-v3 constitution.
- [x] T003 Create feature spec, implementation plan, data model, contracts, quickstart, and tasks under `specs/001-thin-live-canary-path/`.
- [x] T004 Update `AGENTS.md` SPECKIT block to point to `specs/001-thin-live-canary-path/plan.md`.
- [x] T005 Run the active no-mistakes binary's `status` and `runs --limit 5`; when an issue-specific soak binary is active, use the operator-provided override path and record triage result in final handoff and shared soak log if a run exists.
- [x] T006 Verify planning artifacts with `rg -n "(?i:TB[D]|TO[D]O|fix[[:space:]]+later|NE[E]DS[[:space:]]+CLARIFICATION)|\\[[A-Z][A-Z0-9]*(_[A-Z0-9]+)+\\]" .specify/memory/constitution.md specs/001-thin-live-canary-path` and `git diff --check`.

## Phase 2: Production Entrypoint Adoption (US1)

**Goal**: Production binary enters one bolt-v3 build/run path.

**Independent Test**: `cargo test --test bolt_v3_production_entrypoint` fails before implementation because `src/main.rs` still calls `node.run()` directly, then passes after legacy runtime path is removed or made unreachable.

- [x] T007 [US1] Write failing test `tests/bolt_v3_production_entrypoint.rs::main_uses_bolt_v3_runner_wrapper_only` asserting `src/main.rs` contains no production direct `node.run()` and imports/calls `run_bolt_v3_live_node`.
- [x] T008 [US1] Run `cargo test --test bolt_v3_production_entrypoint main_uses_bolt_v3_runner_wrapper_only -- --nocapture`; expected failure references current direct `node.run()` in `src/main.rs`.
- [x] T009 [US1] Refactor `src/main.rs` to load bolt-v3 TOML, validate, build via `build_bolt_v3_live_node`, and run via `run_bolt_v3_live_node`.
- [x] T010 [US1] Remove or isolate legacy production config/ruleset runtime so it cannot be selected in production.
- [x] T011 [US1] Run `cargo test --test bolt_v3_production_entrypoint`, `cargo test --test bolt_v3_live_canary_gate`, and `cargo test --test config_parsing`.
- [x] T012 [US1] Run the active no-mistakes binary's `status`; if unavailable, record that fact instead of blocking the code slice.

## Phase 3: Generic Strategy And Runtime Registration (US3)

**Goal**: Bolt-v3 live-node build path registers configured strategies through a strategy binding, without core concrete strategy leakage.

**Independent Test**: `cargo test --test bolt_v3_strategy_registration` proves injected fake strategy binding can register through core and unsupported strategy fails closed.

- [x] T013 [US3] Write failing tests in `tests/bolt_v3_strategy_registration.rs` for fake binding registration, unsupported strategy rejection, and no concrete strategy key in core registration code.
- [x] T014 [US3] Run `cargo test --test bolt_v3_strategy_registration -- --nocapture`; expected failures show missing bolt-v3 strategy registration surface.
- [x] T015 [US3] Add `src/bolt_v3_strategy_registration.rs` with a generic `StrategyBinding` interface and production binding table.
- [x] T016 [US3] Wire strategy registration into `src/bolt_v3_live_node.rs` after NT client registration and before runner entry.
- [x] T017 [US3] Run `cargo test --test bolt_v3_strategy_registration` and `cargo test --test bolt_v3_provider_binding`.

## Phase 4: Initial Binary-oracle Edge Taker Activation (US3)

**Goal**: Initial taker strategy is configured by reference roles and strategy parameters, not hardcoded provider assumptions.

**Independent Test**: Existing strategy tests plus new config tests prove Polymarket option venue, Chainlink primary reference, and multiple exchange reference roles are configured through TOML.

- [x] T018 [US3] Write failing validation tests proving strategy config accepts multiple exchange reference roles and rejects missing primary oracle/reference roles.
- [x] T019 [US3] Run targeted strategy/config tests; expected failure shows current config cannot express all required reference roles.
- [x] T020 [US3] Extend strategy-archetype validation in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` to validate reference roles generically.
- [x] T021 [US3] Extend fixtures under `tests/fixtures/bolt_v3/` only for operator-visible runtime values, not test-local timing scaffolding.
- [x] T022 [US3] Run `cargo test --test config_parsing` and targeted tests for the initial registered taker strategy.

## Phase 5: Mandatory Decision Evidence (US2)

**Goal**: No submit path can exist without bolt-v3 decision evidence.

**Independent Test**: Strategy construction and submit tests fail closed when evidence writer is absent or persistence fails.

- [x] T023 [US2] Write failing tests that construct the strategy without decision evidence and expect construction rejection.
- [x] T024 [US2] Write failing tests that simulate evidence persistence failure and expect submit rejection before NT submit.
- [x] T025 [US2] Remove optional/fallback evidence submit path from the registered strategy implementation.
- [x] T026 [US2] Make bolt-v3 strategy registration provide mandatory decision evidence.
- [x] T027 [US2] Run targeted strategy tests and source-fence search for fallback direct submit branches.

## Phase 6: Submit Admission Consumes Gate Report (US2)

**Goal**: `BoltV3LiveCanaryGateReport` bounds are enforced before every live submit.

**Independent Test**: `cargo test --test bolt_v3_submit_admission` proves order count, notional cap, cap equality, missing/unarmed report, double-arm/stale-arm, global submit budget, cancel exclusion, and evidence failure all reject before NT submit without consuming admission budget before evidence persists.

- [x] T028 [US2] Write one failing public behavior test in `tests/bolt_v3_submit_admission.rs` for unarmed admission rejecting before NT submit with `NotArmed`.
- [x] T029 [US2] Run `cargo test --test bolt_v3_submit_admission -- --nocapture`; expected failures show missing submit admission module.
- [x] T030 [US2] Add `src/bolt_v3_submit_admission.rs` with shared admission state armed only from `BoltV3LiveCanaryGateReport`.
- [x] T031 [US2] Continue vertical TDD, one behavior at a time, for count cap, notional cap, cap equality, evidence-failure-before-admission, success ordering, global entry/exit/replace-submit budget, cancel exclusion, and double-arm/stale-arm behavior.
- [x] T032 [US2] Wire one shared admission handle from live-node build through strategy contexts into `run_bolt_v3_live_node`, then wire strategy submit calls through decision evidence, submit admission, admission permit, and NT submit.
- [x] T033 [US2] Run `cargo test --test bolt_v3_submit_admission`, targeted strategy submit tests, and source-fence checks across `src/strategies/**/*.rs` and `src/bolt_v3_archetypes/**/*.rs` for direct `submit_order` bypasses.

## Phase 7: Authenticated No-submit Readiness (US4)

**Goal**: Real SSM/venue connect-disconnect produces a redacted report consumed by PR #305 gate.

**Independent Test**: Local tests cover report schema and zero-order guard. Ignored operator test produces real artifact only with explicit approval.

- [x] T034 [US4] Write failing schema tests for no-submit readiness report producer and gate consumer compatibility.
- [x] T035 [US4] Write zero-order source/behavior fence proving readiness code cannot call submit, cancel, replace, or amend order APIs.
- [x] T036 [US4] Implement minimal no-submit readiness runner using existing bolt-v3 build and controlled-connect/disconnect boundaries.
- [x] T037 [US4] Run local readiness tests with mock SSM resolver and no network.
- [ ] T038 [US4] With explicit operator approval, run ignored real SSM/venue no-submit readiness and store redacted report path outside tracked secrets.
- [x] T039 [US4] Run `cargo test --test bolt_v3_live_canary_gate` against the redacted report fixture shape.

## Phase 8: Tiny-capital Live Canary (US5)

**Goal**: One approved capped live order proves production-shaped bolt-v3 spine through NT.

**Independent Test**: Local tests prove all preconditions and fail-closed paths. Operator artifact proves real submit/venue result/cancel/reconciliation.

- [x] T040 [US5] Write failing canary precondition tests requiring exact config checksum, approval id, gate report, submit admission state, and decision evidence.
- [x] T041 [US5] Write ignored operator test or command harness that submits at most one configured canary order after explicit approval.
- [x] T042 [US5] Implement canary operator harness using the production bolt-v3 path and NT adapter submit only.
- [x] T043 [US5] Add strategy-driven cancel path evidence capture for open canary orders.
- [x] T044 [US5] Add restart reconciliation evidence capture through NT adapter state.
- [ ] T045 [US5] Run local fail-closed tests, exact-head CI, no-mistakes triage, and external review after branch is clean and pushed.
- [ ] T046 [US5] With explicit operator approval, run tiny-capital canary and store redacted artifact with exact SHA and config checksum.

## Out Of Scope For MVP

- Backtesting engine.
- Research analytics platform.
- New Bolt-owned venue adapter.
- Bolt-owned order lifecycle or reconciliation implementation.
- Test-literal verifier expansion.

## Execution Order

Phase 1 must merge first. Phases 2-8 are sequential because each removes a live-submit blocker from the prior phase. Do not begin live operations until Phases 2-7 are complete and verified.
