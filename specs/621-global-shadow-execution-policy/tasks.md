# Tasks: Global Shadow Execution Policy

**Input**: `spec.md`, `plan.md`, `research.md`, `contracts/order-execution-policy.md`  
**Gate**: Do not begin implementation tasks until all four adversarial reviews approve.

## Phase 1: Review Gate

- [ ] T001 Commit the plan/spec packet in `specs/621-global-shadow-execution-policy/`
- [ ] T002 Run internal adversarial self-review against `spec.md`, `plan.md`, `research.md`, and `contracts/order-execution-policy.md`
- [ ] T003 Resolve or disprove all internal findings in `internal-adversarial-review.md`
- [ ] T004 Run Gemini adversarial review using `review-prompt.md`
- [ ] T005 Run Grok adversarial review using `review-prompt.md`
- [ ] T006 Run Claude adversarial review using `review-prompt.md`
- [ ] T007 Record unanimous approval or blocker disposition before any source implementation

## Phase 2: Config and Context

- [ ] T008 [US1] Write failing config tests for required root execution mode in `tests/config_parsing.rs`
- [ ] T009 [US1] Write failing config tests rejecting stale strategy-local `parameters.submit_orders` in `tests/config_parsing.rs`
- [ ] T010 [US1] Add root execution-mode config type in `src/bolt_v3_config.rs`
- [ ] T011 [US1] Update root config fixtures in `config/root.toml`, `config/live.local.toml`, and `tests/fixtures/bolt_v3/root.toml`
- [ ] T012 [US2] Extend `StrategyBuildContext` with shared execution policy in `src/strategies/registry.rs`
- [ ] T013 [US2] Build the shared policy from loaded root config in `src/bolt_v3_live_node.rs` and `src/bolt_v3_strategy_registration.rs`

## Phase 3: Shared Execution Routing

- [ ] T014 [US2] Write failing shared live-submit routing test in `tests/bolt_v3_order_execution.rs`
- [ ] T015 [US2] Write failing shared shadow-submit routing test in `tests/bolt_v3_order_execution.rs`
- [ ] T016 [US2] Write failing shared live-cancel and shadow-cancel routing tests in `tests/bolt_v3_order_execution.rs`
- [ ] T017 [US2] Create `src/bolt_v3_order_execution.rs` with execution mode, policy, shared submit context, submit routing, and cancel routing
- [ ] T018 [US2] Export `bolt_v3_order_execution` from `src/lib.rs`

## Phase 4: Strategy Migration

- [ ] T019 [US2] Move `SubmitContext` usage from `src/strategies/binary_oracle_edge_taker/mod.rs` to the shared module
- [ ] T020 [US2] Replace strategy-local submit gating with shared submit routing in `src/strategies/binary_oracle_edge_taker/mod.rs`
- [ ] T021 [US2] Replace strategy-local cancel gating with shared cancel routing in `src/strategies/binary_oracle_edge_taker/mod.rs`
- [ ] T022 [US1] Remove `submit_orders` from `src/strategies/binary_oracle_edge_taker/config.rs`
- [ ] T023 [US1] Remove `submit_orders` from `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- [ ] T024 [US1] Remove `submit_orders` from strategy TOML files under `config/strategies/` and `tests/fixtures/bolt_v3/strategies/`
- [ ] T025 [US2] Add source-level guard test proving production strategy code does not read `submit_orders`
- [ ] T026 [US4] Update existing shadow-mode evidence tests to configure shadow through `StrategyBuildContext`

## Phase 5: Managed Venue-Action Guard

- [ ] T027 [US3] Write failing validation tests for shadow mode plus `manage_stop = true`
- [ ] T028 [US3] Write failing validation tests for shadow mode plus `manage_gtd_expiry = true`
- [ ] T029 [US3] Write failing validation tests for shadow mode plus `manage_contingent_orders = true`
- [ ] T030 [US3] Write failing validation tests for shadow mode plus non-empty `external_order_claims`
- [ ] T031 [US3] Implement shared validation in `src/bolt_v3_validate.rs`

## Phase 6: Docs, Fences, Verification

- [ ] T032 [US1] Update active schema docs in `docs/bolt-v3/2026-04-25-bolt-v3-schema.md`
- [ ] T033 [US2] Update schema-current verifier tests in `scripts/test_verify_bolt_v3_schema_current.py`
- [ ] T034 [US2] Update schema-current verifier in `scripts/verify_bolt_v3_schema_current.py`
- [ ] T035 [US2] Update runtime-literal audit and verifier fixtures for new diagnostics
- [ ] T036 Run local non-compile verification listed in `quickstart.md`
- [ ] T037 Commit implementation
- [ ] T038 Push branch and open or update a draft PR
- [ ] T039 Run `just verify-remote` and record exact-head CI evidence
