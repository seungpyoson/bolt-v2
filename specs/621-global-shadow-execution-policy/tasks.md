# Tasks: Global Shadow Execution Policy

**Input**: `spec.md`, `plan.md`, `research.md`, `contracts/order-execution-policy.md`
**Gate**: Do not begin implementation tasks until all four adversarial reviews approve.

## Phase 1: Review Gate

- [X] T001 Commit the plan/spec packet in `specs/621-global-shadow-execution-policy/`
- [X] T002 Run internal adversarial self-review against `spec.md`, `plan.md`, `research.md`, and `contracts/order-execution-policy.md`
- [X] T003 Resolve or disprove all internal findings in `internal-adversarial-review.md`
- [X] T004 Run Gemini adversarial review using `review-prompt.md`
- [X] T005 Run Grok adversarial review using `review-prompt.md`
- [X] T006 Run Claude adversarial review using `review-prompt.md`
- [X] T007 Record unanimous approval or blocker disposition before any source implementation

## Phase 2: Config and Context

- [X] T008 [US1] Write failing config tests for required root execution mode in `tests/config_parsing.rs`
- [X] T009 [US1] Write failing config tests rejecting stale strategy-local `parameters.submit_orders` in `tests/config_parsing.rs`
- [X] T010 [US1] Add root execution-mode config type in `src/bolt_v3_config.rs`
- [X] T011 [US1] Update root config fixtures in `config/root.toml`, `config/live.local.toml`, and `tests/fixtures/bolt_v3/root.toml`
- [X] T012 [US2] Extend `StrategyBuildContext` with shared execution policy in `src/strategies/registry.rs`
- [X] T013 [US2] Build the shared policy from loaded root config in `src/bolt_v3_live_node.rs` and `src/bolt_v3_strategy_registration.rs`

## Phase 3: Shared Execution Routing

- [X] T014 [US2] Write failing shared live-submit routing test in `tests/bolt_v3_order_execution.rs`
- [X] T015 [US2] Write failing shared shadow-submit routing test in `tests/bolt_v3_order_execution.rs`
- [X] T016 [US2] Write failing shared live-cancel and shadow-cancel routing tests in `tests/bolt_v3_order_execution.rs`
- [X] T017 [US2] Write failing source-fence/static verifier tests rejecting direct strategy calls to NT venue mutation APIs outside `src/bolt_v3_order_execution.rs`
- [X] T018 [US2] Create `src/bolt_v3_order_execution.rs` with execution mode, policy, shared submit context, submit routing, and cancel routing
- [X] T019 [US2] Export `bolt_v3_order_execution` from `src/lib.rs`

## Phase 4: Strategy Migration

- [X] T020 [US2] Move `SubmitContext` usage from `src/strategies/binary_oracle_edge_taker/mod.rs` to the shared module
- [X] T021 [US2] Replace strategy-local submit gating with shared submit routing in `src/strategies/binary_oracle_edge_taker/mod.rs`
- [X] T022 [US2] Replace strategy-local cancel gating with shared cancel routing in `src/strategies/binary_oracle_edge_taker/mod.rs`
- [X] T023 [US1] Remove `submit_orders` from `src/strategies/binary_oracle_edge_taker/config.rs`
- [X] T024 [US1] Remove `submit_orders` from `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- [X] T025 [US1] Remove `submit_orders` from strategy TOML files under `config/strategies/` and `tests/fixtures/bolt_v3/strategies/`
- [X] T026 [US2] Add source-level guard proving production strategy code neither reads `submit_orders` nor directly calls `submit_order`, `submit_order_list`, `modify_order`, `cancel_order`, `cancel_orders`, `cancel_all_orders`, `close_position`, or `close_all_positions`
- [X] T027 [US4] Add an integration-level shadow wiring test proving strategy evidence/admission reaches the shared path without invoking the NT submit closure
- [X] T028 [US4] Update existing shadow-mode evidence tests to configure shadow through `StrategyBuildContext`

## Phase 5: Managed Venue-Action Guard

- [X] T029 [US3] Write failing validation tests for shadow mode plus `manage_stop = true`
- [X] T030 [US3] Write failing validation tests for shadow mode plus `manage_gtd_expiry = true`
- [X] T031 [US3] Write failing validation tests for shadow mode plus `manage_contingent_orders = true`
- [X] T032 [US3] Write failing validation tests for shadow mode plus non-empty `external_order_claims`
- [X] T033 [US3] Document the pinned NT `StrategyConfig` audit proving other fields are not independent venue-mutation enablers
- [X] T034 [US3] Implement shared validation in `src/bolt_v3_validate.rs`

## Phase 6: Docs, Fences, Verification

- [X] T035 [US1] Update active schema docs in `docs/bolt-v3/2026-04-25-bolt-v3-schema.md`
- [X] T036 [US2] Update schema-current verifier tests in `scripts/test_verify_bolt_v3_schema_current.py`
- [X] T037 [US2] Update schema-current verifier in `scripts/verify_bolt_v3_schema_current.py`
- [X] T038 [US2] Update runtime-literal audit and verifier fixtures for new diagnostics
- [X] T039 Run local non-compile verification listed in `quickstart.md`
- [X] T040 Commit implementation
- [X] T041 Push branch and open or update a draft PR
- [X] T042 Run `just verify-remote` and record exact-head CI evidence
