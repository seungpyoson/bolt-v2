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
- [ ] T008 [US1] Add generic fee-provider resolver data structures and existing-registry dispatch from strategy `execution_client_id` to loaded `clients.<id>.venue` provider key in `src/bolt_v3_providers/mod.rs`
- [ ] T009 [US1] Define `ProviderBinding::build_fee_provider` with provider-agnostic inputs: client key, provider-specific config, and a borrowed or shared resolved secrets snapshot reference in `src/bolt_v3_providers/mod.rs`
- [ ] T010 [US1] Register the Polymarket fee-provider builder through its provider binding in `src/bolt_v3_providers/mod.rs`
- [ ] T011 [US1] Keep concrete Polymarket construction inside `src/bolt_v3_providers/polymarket.rs`
- [ ] T012 [US1] Run `cargo test --lib bolt_v3_providers::tests` and `cargo test --lib polymarket_fee_provider -- --nocapture`

**Checkpoint**: Provider layer can resolve a strategy fee provider, and the source-fence remains red until archetype code stops naming Polymarket construction.

---

## Phase 3: User Story 1 - Generic Fee-Provider Resolution (Priority: P1)

**Goal**: `binary_oracle_edge_taker` runtime registration receives a fee provider through a generic execution-client/provider capability boundary.

**Independent Test**: Configured Polymarket runtime registration still succeeds after the archetype stops calling `polymarket::build_fee_provider` directly.

### Tests for User Story 1

- [ ] T013 [US1] Add a failing `binary_oracle_registration_resolves_fee_provider_through_provider_boundary` runtime registration test proving fee-provider resolution follows the provider boundary in `tests/bolt_v3_strategy_registration.rs`
- [ ] T014 [US1] Replace direct `polymarket::build_fee_provider` usage and imports with generic provider resolution in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- [ ] T015 [US1] Preserve execution-client validation error coverage and add resolver error coverage for missing execution client id, unsupported provider kind, valid execution client with no fee-provider binding, provider-specific config parse failure, missing or invalid resolved secret binding, and provider-specific client construction failure in `tests/bolt_v3_strategy_registration.rs`
- [ ] T016 [US1] Add a sentinel-secret error-format test proving resolver and binding `Display`/`Debug` output does not contain raw secret material in `tests/bolt_v3_strategy_registration.rs`
- [ ] T017 [US1] Add a guard test proving fee-provider resolution does not call `FeeProvider::warm(...)` during registration in `tests/bolt_v3_strategy_registration.rs`
- [ ] T018 [US1] Run `bolt_v3_live_node_build_registers_configured_binary_oracle_strategy` and `binary_oracle_registration_resolves_fee_provider_through_provider_boundary` in `tests/bolt_v3_strategy_registration.rs`
- [ ] T019 [US1] Run targeted runtime registration tests in `tests/bolt_v3_strategy_registration.rs`

**Checkpoint**: US1 passes with unchanged Polymarket behavior and no order-intent/admission changes.

---

## Phase 4: User Story 2 - Shared Layers Stay Venue-Agnostic (Priority: P2)

**Goal**: Source boundaries keep concrete venue logic in provider modules, not strategy/archetype registration or shared runtime core.

**Independent Test**: A source-fence test fails if `binary_oracle_edge_taker` calls `polymarket::build_fee_provider` again.

### Tests for User Story 2

- [ ] T020 [US2] Confirm `fee_provider_source_fence_blocks_concrete_provider_in_shared_layers` now passes for every file under `src/bolt_v3_archetypes/`, strategy modules under `src/strategies/`, `src/bolt_v3_strategy_registration.rs`, `src/bolt_v3_submit_admission.rs`, and `src/bolt_v3_order_intent.rs` in `tests/bolt_v3_strategy_registration.rs`
- [ ] T021 [US2] Keep concrete provider import allowances scoped to `src/bolt_v3_providers/mod.rs` and `src/bolt_v3_providers/polymarket.rs`
- [ ] T022 [US2] Update `specs/453-fee-provider-decoupling/research.md` only for implementation evidence, not for order-intent or #451 scope expansion

**Checkpoint**: US2 proves the shared registration layer is venue-agnostic.

---

## Phase 5: Verification And Review

**Purpose**: Prove the final exact head before PR review.

- [ ] T023 Run `cargo fmt -- --check`
- [ ] T024 Run targeted Rust tests for `tests/bolt_v3_strategy_registration.rs`
- [ ] T025 Run targeted Rust tests for `src/bolt_v3_providers/mod.rs`
- [ ] T026 Run `cargo test --locked` and verify every `quickstart.md` `cargo test` filter matches at least one implemented test function
- [ ] T027 Run `just clippy`
- [ ] T028 Run the ai-slop-cleaner skill against the final diff before requesting review
- [ ] T029 Open a PR for issue #453 only
- [ ] T030 Confirm exact PR head CI is green
- [ ] T031 Request external exact-head review after all local checks pass and PR CI is green

---

## Dependencies & Execution Order

- Phase 1 blocks all runtime edits.
- Phase 2 creates the red source-fence and provider-boundary tests before production edits.
- US1 must complete before US2 source-fence cleanup is trusted.
- Phase 5 starts only after US1 and US2 pass locally.
- No task changes PR #434 order-intent semantics or implements #451 generic wrapper extraction.
