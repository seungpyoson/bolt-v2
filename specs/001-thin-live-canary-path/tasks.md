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
- [x] T005 Run `/private/tmp/no-mistakes-soak-bin status` and `/private/tmp/no-mistakes-soak-bin runs --limit 5`; record triage result in final handoff and shared soak log if a run exists.
- [x] T006 Verify planning artifacts with `rg -n "(?i:TB[D]|TO[D]O|fix[[:space:]]+later|NE[E]DS[[:space:]]+CLARIFICATION)|\\[[A-Z][A-Z0-9]*(_[A-Z0-9]+)+\\]" .specify/memory/constitution.md specs/001-thin-live-canary-path` and `git diff --check`.

## Phase 2: Production Entrypoint Adoption (US1)

**Goal**: Production binary enters one bolt-v3 build/run path.

**Independent Test**: `cargo test --test bolt_v3_production_entrypoint` fails before implementation because `src/main.rs` still calls `node.run()` directly, then passes after legacy runtime path is removed or made unreachable.

- [ ] T007 [US1] Write failing test `tests/bolt_v3_production_entrypoint.rs::main_uses_bolt_v3_runner_wrapper_only` asserting `src/main.rs` contains no production direct `node.run()` and imports/calls `run_bolt_v3_live_node`.
- [ ] T008 [US1] Run `cargo test --test bolt_v3_production_entrypoint main_uses_bolt_v3_runner_wrapper_only -- --nocapture`; expected failure references current direct `node.run()` in `src/main.rs`.
- [ ] T009 [US1] Refactor `src/main.rs` to load bolt-v3 TOML, validate, build via `build_bolt_v3_live_node`, and run via `run_bolt_v3_live_node`.
- [ ] T010 [US1] Remove or isolate legacy production config/ruleset runtime so it cannot be selected in production.
- [ ] T011 [US1] Run `cargo test --test bolt_v3_production_entrypoint`, `cargo test --test bolt_v3_live_canary_gate`, and `cargo test --test config_parsing`.
- [ ] T012 [US1] Run `/private/tmp/no-mistakes-soak-bin status` and capture whether an active run or prior error code is shown.

## Phase 3: Generic Strategy And Runtime Registration (US3)

**Goal**: Bolt-v3 live-node build path registers configured strategies through a strategy binding, without core concrete strategy leakage.

**Independent Test**: `cargo test --test bolt_v3_strategy_registration` proves injected fake strategy binding can register through core and unsupported strategy fails closed.

- [ ] T013 [US3] Write failing tests in `tests/bolt_v3_strategy_registration.rs` for fake binding registration, unsupported strategy rejection, and no concrete strategy key in core registration code.
- [ ] T014 [US3] Run `cargo test --test bolt_v3_strategy_registration -- --nocapture`; expected failures show missing bolt-v3 strategy registration surface.
- [ ] T015 [US3] Add `src/bolt_v3_strategy_registration.rs` with a generic `StrategyBinding` interface and production binding table.
- [ ] T016 [US3] Wire strategy registration into `src/bolt_v3_live_node.rs` after NT client registration and before runner entry.
- [ ] T017 [US3] Run `cargo test --test bolt_v3_strategy_registration` and `cargo test --test bolt_v3_provider_binding`.

## Phase 4: Initial Binary-oracle Edge Taker Activation (US3)

**Goal**: Initial taker strategy is configured by reference roles and strategy parameters, not hardcoded provider assumptions.

**Independent Test**: Existing strategy tests plus new config tests prove Polymarket option venue, Chainlink primary reference, and multiple exchange reference roles are configured through TOML.

- [ ] T018 [US3] Write failing validation tests proving strategy config accepts multiple exchange reference roles and rejects missing primary oracle/reference roles.
- [ ] T019 [US3] Run targeted strategy/config tests; expected failure shows current config cannot express all required reference roles.
- [ ] T020 [US3] Extend strategy-archetype validation in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` to validate reference roles generically.
- [ ] T021 [US3] Extend fixtures under `tests/fixtures/bolt_v3/` only for operator-visible runtime values, not test-local timing scaffolding.
- [ ] T022 [US3] Run `cargo test --test config_parsing` and targeted `eth_chainlink_taker` strategy tests.

## Phase 5: Mandatory Decision Evidence (US2)

**Goal**: No submit path can exist without bolt-v3 decision evidence.

**Independent Test**: Strategy construction and submit tests fail closed when evidence writer is absent or persistence fails.

- [ ] T023 [US2] Write failing tests that construct the strategy without decision evidence and expect construction rejection.
- [ ] T024 [US2] Write failing tests that simulate evidence persistence failure and expect submit rejection before NT submit.
- [ ] T025 [US2] Remove optional/fallback evidence submit path from `src/strategies/eth_chainlink_taker.rs`.
- [ ] T026 [US2] Make bolt-v3 strategy registration provide mandatory decision evidence.
- [ ] T027 [US2] Run targeted strategy tests and source-fence search for fallback direct submit branches.

## Phase 6: Submit Admission Consumes Gate Report (US2)

**Goal**: `LiveCanaryGateReport` bounds are enforced before every live submit.

**Independent Test**: `cargo test --test bolt_v3_submit_admission` proves order count, notional cap, missing report, and missing evidence all reject before NT submit.

- [ ] T028 [US2] Write failing tests in `tests/bolt_v3_submit_admission.rs` for one-order cap, over-notional rejection, missing gate report, and missing evidence.
- [ ] T029 [US2] Run `cargo test --test bolt_v3_submit_admission -- --nocapture`; expected failures show missing submit admission module.
- [ ] T030 [US2] Add `src/bolt_v3_submit_admission.rs` with config-derived admission state initialized from `BoltV3LiveCanaryGateReport`.
- [ ] T031 [US2] Wire strategy submit calls through submit admission before NT submit.
- [ ] T032 [US2] Run `cargo test --test bolt_v3_submit_admission`, targeted strategy submit tests, and source-fence checks for direct `submit_order` bypasses.

## Phase 7: Authenticated No-submit Readiness (US4)

**Goal**: Real SSM/venue connect-disconnect produces a redacted report consumed by PR #305 gate.

**Independent Test**: Local tests cover report schema and zero-order guard. Ignored operator test produces real artifact only with explicit approval.

- [ ] T033 [US4] Write failing schema tests for no-submit readiness report producer and gate consumer compatibility.
- [ ] T034 [US4] Write zero-order source/behavior fence proving readiness code cannot call submit, cancel, replace, or amend order APIs.
- [ ] T035 [US4] Implement minimal no-submit readiness runner using existing bolt-v3 build and controlled-connect/disconnect boundaries.
- [ ] T036 [US4] Run local readiness tests with mock SSM resolver and no network.
- [ ] T037 [US4] With explicit operator approval, run ignored real SSM/venue no-submit readiness and store redacted report path outside tracked secrets.
- [ ] T038 [US4] Run `cargo test --test bolt_v3_live_canary_gate` against the redacted report fixture shape.

## Phase 8: Tiny-capital Live Canary (US5)

**Goal**: One approved capped live order proves production-shaped bolt-v3 spine through NT.

**Independent Test**: Local tests prove all preconditions and fail-closed paths. Operator artifact proves real submit/venue result/cancel/reconciliation.

- [ ] T039 [US5] Write failing canary precondition tests requiring exact config checksum, approval id, gate report, submit admission state, and decision evidence.
- [ ] T040 [US5] Write ignored operator test or command harness that submits at most one configured canary order after explicit approval.
- [ ] T041 [US5] Implement canary operator harness using the production bolt-v3 path and NT adapter submit only.
- [ ] T042 [US5] Add strategy-driven cancel path evidence capture for open canary orders.
- [ ] T043 [US5] Add restart reconciliation evidence capture through NT adapter state.
- [ ] T044 [US5] Run local fail-closed tests, exact-head CI, no-mistakes triage, and external review after branch is clean and pushed.
- [ ] T045 [US5] With explicit operator approval, run tiny-capital canary and store redacted artifact with exact SHA and config checksum.

## Out Of Scope For MVP

- Backtesting engine.
- Research analytics platform.
- New Bolt-owned venue adapter.
- Bolt-owned order lifecycle or reconciliation implementation.
- Test-literal verifier expansion.

## Execution Order

Phase 1 must merge first. Phases 2-8 are sequential because each removes a live-submit blocker from the prior phase. Do not begin live operations until Phases 2-7 are complete and verified.
