# Tasks: Fee-Provider Binding Decoupling

**Input**: Design documents from `/specs/453-fee-provider-decoupling/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/fee-provider-resolution.md`
**Tests**: Required by spec; use red-green-refactor, one behavior at a time.

## Phase 1: Pre-Implementation Gates

**Purpose**: Confirm scope and approval before runtime code changes.

- [ ] T001 Confirm user approval of `specs/453-fee-provider-decoupling/spec.md`, `specs/453-fee-provider-decoupling/plan.md`, and `specs/453-fee-provider-decoupling/tasks.md`
- [ ] T002 Request external plan review against `specs/453-fee-provider-decoupling/spec.md`, `specs/453-fee-provider-decoupling/plan.md`, `specs/453-fee-provider-decoupling/research.md`, and `specs/453-fee-provider-decoupling/contracts/fee-provider-resolution.md`, then record reviewer jobs and verdicts in the implementation handoff or PR body
- [ ] T003 Record approved scope boundary that order-intent semantics and #451 generic submission wrapper remain out of scope, and confirm research line references still match current main, in `specs/453-fee-provider-decoupling/research.md`

---

## Phase 2: Foundational

**Purpose**: Prove the current direct coupling and define the generic provider capability boundary before touching the archetype registration path.

- [ ] T004 [US2] Add a failing `fee_provider_source_fence_blocks_concrete_provider_in_shared_layers` deterministic source-fence test that scans every file under `src/bolt_v3_archetypes/`, strategy modules under `src/strategies/`, `src/bolt_v3_strategy_registration.rs`, `src/bolt_v3_submit_admission.rs`, and `src/bolt_v3_order_intent.rs` for forbidden concrete-provider references in `tests/bolt_v3_strategy_registration.rs`
- [ ] T005 [US2] Run `fee_provider_source_fence_blocks_concrete_provider_in_shared_layers` and confirm it fails on the current direct Polymarket archetype call before production edits
- [ ] T006 [US1] Add a failing `fee_provider_resolution_uses_provider_binding_registry` provider-binding unit test for fee-provider resolution through `ProviderBinding` in `src/bolt_v3_providers/mod.rs`
- [ ] T007 [US1] Run `fee_provider_resolution_uses_provider_binding_registry` and confirm it fails before resolver implementation
- [ ] T008 [US1] Add a failing `fee_provider_resolution_rejects_unsupported_provider_kind` resolver-boundary test in `src/bolt_v3_providers/mod.rs`
- [ ] T009 [US1] Run `fee_provider_resolution_rejects_unsupported_provider_kind` and confirm it fails before resolver implementation
- [ ] T010 [US1] Add a failing `fee_provider_resolution_rejects_provider_without_fee_binding` resolver-boundary test in `src/bolt_v3_providers/mod.rs`
- [ ] T011 [US1] Run `fee_provider_resolution_rejects_provider_without_fee_binding` and confirm it fails before resolver implementation
- [ ] T012 [US1] Add a failing `fee_provider_resolution_reports_provider_config_parse_failure` resolver-boundary test in `src/bolt_v3_providers/mod.rs`
- [ ] T013 [US1] Run `fee_provider_resolution_reports_provider_config_parse_failure` and confirm it fails before resolver implementation
- [ ] T014 [US1] Add a failing `fee_provider_resolution_rejects_invalid_secret_binding` resolver-boundary test in `src/bolt_v3_providers/mod.rs`
- [ ] T015 [US1] Run `fee_provider_resolution_rejects_invalid_secret_binding` and confirm it fails before resolver implementation
- [ ] T016 [US1] Add a failing `fee_provider_resolution_reports_provider_client_construction_failure` resolver-boundary test in `src/bolt_v3_providers/mod.rs`
- [ ] T017 [US1] Run `fee_provider_resolution_reports_provider_client_construction_failure` and confirm it fails before resolver implementation
- [ ] T018 [US1] Add a failing `fee_provider_resolution_error_display_debug_redacts_sentinel_secret` test proving resolver and binding `Display`/`Debug` output does not contain raw secret material in `src/bolt_v3_providers/mod.rs`
- [ ] T019 [US1] Run `fee_provider_resolution_error_display_debug_redacts_sentinel_secret` and confirm it fails before resolver implementation
- [ ] T020 [US1] Add a failing `fee_provider_resolution_does_not_warm_during_registration` guard test proving resolver construction does not call `FeeProvider::warm(...)` in `tests/bolt_v3_strategy_registration.rs`
- [ ] T021 [US1] Run `fee_provider_resolution_does_not_warm_during_registration` and confirm it fails before resolver implementation
- [ ] T022 [US1] Add generic fee-provider resolver data structures and existing-registry dispatch from strategy `execution_client_id` to loaded `clients.<id>.venue` provider key in `src/bolt_v3_providers/mod.rs`
- [ ] T023 [US1] Define `ProviderBinding::build_fee_provider` with provider-agnostic inputs: client key, provider-specific config, and a borrowed or shared resolved secrets snapshot reference in `src/bolt_v3_providers/mod.rs`
- [ ] T024 [US1] Register the Polymarket fee-provider builder through its provider binding in `src/bolt_v3_providers/mod.rs`
- [ ] T025 [US1] Keep concrete Polymarket construction inside `src/bolt_v3_providers/polymarket*`
- [ ] T026 [US1] Run `cargo test --lib bolt_v3_providers::tests` and `cargo test --test bolt_v3_strategy_registration fee_provider_resolution_does_not_warm_during_registration -- --nocapture`

**Checkpoint**: Provider layer can resolve a strategy fee provider, and the source-fence remains red until archetype code stops naming Polymarket construction.

---

## Phase 3: User Story 1 - Generic Fee-Provider Resolution (Priority: P1)

**Goal**: `binary_oracle_edge_taker` runtime registration receives a fee provider through a generic execution-client/provider capability boundary.

**Independent Test**: Configured Polymarket runtime registration still succeeds after the archetype stops calling `polymarket::build_fee_provider` directly.

### Tests for User Story 1

- [ ] T027 [US1] Add a failing `binary_oracle_registration_resolves_fee_provider_through_provider_boundary` runtime registration test proving fee-provider resolution follows the provider boundary in `tests/bolt_v3_strategy_registration.rs`
- [ ] T028 [US1] Run `binary_oracle_registration_resolves_fee_provider_through_provider_boundary` and confirm it fails before archetype production edits in `tests/bolt_v3_strategy_registration.rs`
- [ ] T029 [US1] Replace direct `polymarket::build_fee_provider` usage and imports with generic provider resolution in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- [ ] T030 [US1] Preserve existing execution-client validation coverage, including missing execution client id, while re-running the resolver-boundary error tests added in Phase 2 in `src/bolt_v3_providers/mod.rs` and `tests/bolt_v3_strategy_registration.rs`
- [ ] T031 [US1] Run `bolt_v3_live_node_build_registers_configured_binary_oracle_strategy`, `binary_oracle_registration_resolves_fee_provider_through_provider_boundary`, and targeted runtime registration tests in `tests/bolt_v3_strategy_registration.rs`

**Checkpoint**: US1 passes with unchanged Polymarket behavior and no order-intent/admission changes.

---

## Phase 4: User Story 2 - Shared Layers Stay Venue-Agnostic (Priority: P2)

**Goal**: Source boundaries keep concrete venue logic in provider modules, not strategy/archetype registration or shared runtime core.

**Independent Test**: A source-fence test fails if `binary_oracle_edge_taker` calls `polymarket::build_fee_provider` again.

### Tests for User Story 2

- [ ] T032 [US2] Confirm `fee_provider_source_fence_blocks_concrete_provider_in_shared_layers` now passes for every file under `src/bolt_v3_archetypes/`, strategy modules under `src/strategies/`, `src/bolt_v3_strategy_registration.rs`, `src/bolt_v3_submit_admission.rs`, and `src/bolt_v3_order_intent.rs` in `tests/bolt_v3_strategy_registration.rs`
- [ ] T033 [US2] Keep concrete provider import allowances scoped to `src/bolt_v3_providers/mod.rs` and `src/bolt_v3_providers/polymarket*`, with `src/bolt_v3_providers/mod.rs` limited to registry wiring only
- [ ] T034 [US2] Update `specs/453-fee-provider-decoupling/research.md` only for implementation evidence, not for order-intent or #451 scope expansion

**Checkpoint**: US2 proves the shared registration layer is venue-agnostic.

---

## Phase 5: Verification And Review

**Purpose**: Prove the final exact head before PR review.

- [ ] T035 Run `cargo fmt -- --check`
- [ ] T036 Run targeted Rust tests for `tests/bolt_v3_strategy_registration.rs`
- [ ] T037 Run targeted Rust tests for `src/bolt_v3_providers/mod.rs`
- [ ] T038 Run `cargo test --locked` and verify every `quickstart.md` `cargo test` filter matches at least one implemented test function
- [ ] T039 Run `just clippy`
- [ ] T040 Run the ai-slop-cleaner skill against the final diff before requesting review
- [ ] T041 Open a PR for issue #453 only
- [ ] T042 Confirm exact PR head CI is green
- [ ] T043 Request external exact-head review after all local checks pass and PR CI is green

---

## Dependencies & Execution Order

- Phase 1 blocks all runtime edits.
- Phase 2 creates the red source-fence, provider-boundary, error-taxonomy, redaction, and no-warm tests before resolver production edits.
- US1 creates and runs the runtime registration red test before archetype production edits.
- US1 must complete before US2 source-fence cleanup is trusted.
- Phase 5 starts only after US1 and US2 pass locally.
- No task changes PR #434 order-intent semantics or implements #451 generic wrapper extraction.
