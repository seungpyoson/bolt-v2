# Tasks: Quote-Quantity SELL Limit Admission

**Input**: Design documents from `/specs/452-quote-quantity-sell-admission/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/quote-quantity-admission.md`
**Tests**: Required by spec; use red-green-refactor, one behavior at a time.

## Phase 1: Pre-Implementation Gates

**Purpose**: Confirm scope and approval before runtime code changes.

- [ ] T001 Confirm user approval of `specs/452-quote-quantity-sell-admission/spec.md`, `specs/452-quote-quantity-sell-admission/plan.md`, and `specs/452-quote-quantity-sell-admission/tasks.md`
- [ ] T002 Request external plan review against `specs/452-quote-quantity-sell-admission/spec.md`, `specs/452-quote-quantity-sell-admission/plan.md`, `specs/452-quote-quantity-sell-admission/research.md`, and `specs/452-quote-quantity-sell-admission/contracts/quote-quantity-admission.md`, then record reviewer jobs and verdicts in the implementation handoff or PR body
- [ ] T003 Record approved scope boundary that #451 generic wrapper extraction remains out of scope in `specs/452-quote-quantity-sell-admission/research.md`

---

## Phase 2: Foundational

**Purpose**: Prove latent SELL admission bugs before extracting or wiring any helper.

- [ ] T004 [US1] Add a failing non-inverse `quote_quantity_sell_limit_submit_admission_floors_to_quote_quantity` strategy regression for `bid > limit_price` in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T005 [US1] Run `quote_quantity_sell_limit_submit_admission_floors_to_quote_quantity` and confirm it fails before production edits in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T006 [US1] Add a failing `quote_quantity_sell_limit_missing_quote_uses_submitted_quote_quantity` strategy regression in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T007 [US1] Run `quote_quantity_sell_limit_missing_quote_uses_submitted_quote_quantity` and confirm it fails before production edits in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T008 [US1] Add a failing `quote_quantity_sell_limit_missing_context_fails_closed` strategy regression in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T009 [US1] Run `quote_quantity_sell_limit_missing_context_fails_closed` and confirm it fails before production edits in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T010 [US1] Add a failing non-inverse `quote_quantity_sell_stop_limit_submit_admission_floors_to_quote_quantity` strategy regression in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T011 [US1] Run `quote_quantity_sell_stop_limit_submit_admission_floors_to_quote_quantity` and confirm it fails before production edits in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T012 [US1] Add a failing `quote_quantity_sell_stop_limit_missing_quote_uses_submitted_quote_quantity` strategy regression in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T013 [US1] Run `quote_quantity_sell_stop_limit_missing_quote_uses_submitted_quote_quantity` and confirm it fails before production edits in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T014 [US1] Add a failing `quote_quantity_sell_stop_limit_missing_context_fails_closed` strategy regression in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T015 [US1] Run `quote_quantity_sell_stop_limit_missing_context_fails_closed` and confirm it fails before production edits in `src/strategies/binary_oracle_edge_taker.rs`

**Checkpoint**: Red strategy-level regressions prove the need for admission changes before any shared helper is introduced or wired.

---

## Phase 3: User Story 1 - Conservative Admission Contract (Priority: P1)

**Goal**: Quote-quantity SELL Limit/StopLimit admission cannot silently understate notional when market data makes effective price exceed order price.

**Independent Test**: A quote-quantity SELL Limit order with `bid > limit_price` admits at no less than submitted quote quantity.

### Tests for User Story 1

- [ ] T016 [US1] Add a failing `quote_quantity_sell_limit_helper_floors_to_submitted_quote_quantity` helper test in `tests/bolt_v3_submit_admission.rs`, including a fractional fixture that proves the floor uses `rust_decimal::Decimal` values instead of `f64` or string comparison
- [ ] T017 [US1] Run `quote_quantity_sell_limit_helper_floors_to_submitted_quote_quantity` and confirm it fails before helper implementation in `tests/bolt_v3_submit_admission.rs`
- [ ] T018 [US1] Implement the pure conservative helper for SELL Limit floor behavior for non-inverse inputs only, using the existing `rust_decimal::Decimal` dependency for parsed price, quantity, calculated notional, submitted quote quantity, and `Decimal::max` floor comparison, keeping inverse instruments on the existing NT-derived path until explicit inverse coverage lands, in `src/bolt_v3_submit_admission.rs`
- [ ] T019 [US1] Wire SELL Limit `submit_admission_request_from_order` through the conservative helper in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T020 [US1] Run `quote_quantity_sell_limit_submit_admission_floors_to_quote_quantity`, `quote_quantity_sell_limit_helper_floors_to_submitted_quote_quantity`, and the existing BUY/Market admission regressions in `src/strategies/binary_oracle_edge_taker.rs` and `tests/bolt_v3_submit_admission.rs`
- [ ] T021 [US1] Add a failing `quote_quantity_sell_limit_helper_missing_quote_uses_submitted_quote_quantity` helper test in `tests/bolt_v3_submit_admission.rs`
- [ ] T022 [US1] Run `quote_quantity_sell_limit_helper_missing_quote_uses_submitted_quote_quantity` and confirm it fails before fallback implementation in `tests/bolt_v3_submit_admission.rs`
- [ ] T023 [US1] Implement the SELL Limit missing-quote-cache fallback in the conservative helper in `src/bolt_v3_submit_admission.rs`
- [ ] T024 [US1] Add a failing `quote_quantity_sell_limit_helper_missing_context_fails_closed` helper test in `tests/bolt_v3_submit_admission.rs` covering both missing instrument context and decimal parse failure
- [ ] T025 [US1] Run `quote_quantity_sell_limit_helper_missing_context_fails_closed` and confirm it fails before fail-closed implementation in `tests/bolt_v3_submit_admission.rs`
- [ ] T026 [US1] Implement the SELL Limit missing-context fail-closed guard in `src/bolt_v3_submit_admission.rs`
- [ ] T027 [US1] Re-run SELL Limit missing-quote/missing-context strategy and helper regressions plus existing BUY/Market admission regressions in `src/strategies/binary_oracle_edge_taker.rs` and `tests/bolt_v3_submit_admission.rs`
- [ ] T028 [US1] Add a failing `quote_quantity_sell_stop_limit_helper_floors_to_submitted_quote_quantity` helper test in `tests/bolt_v3_submit_admission.rs`, including a fractional fixture that proves the floor uses `rust_decimal::Decimal` values instead of `f64` or string comparison
- [ ] T029 [US1] Run `quote_quantity_sell_stop_limit_helper_floors_to_submitted_quote_quantity` and confirm it fails before StopLimit floor implementation in `tests/bolt_v3_submit_admission.rs`
- [ ] T030 [US1] Extend the conservative floor path to SELL StopLimit orders for non-inverse inputs only, preserving the same `rust_decimal::Decimal` parse and `Decimal::max` floor domain, in `src/bolt_v3_submit_admission.rs`
- [ ] T031 [US1] Wire SELL StopLimit `submit_admission_request_from_order` through the conservative helper in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T032 [US1] Run SELL StopLimit floor strategy/helper regressions plus existing BUY/Market admission regressions in `src/strategies/binary_oracle_edge_taker.rs` and `tests/bolt_v3_submit_admission.rs`
- [ ] T033 [US1] Add a failing `quote_quantity_sell_stop_limit_helper_missing_quote_uses_submitted_quote_quantity` helper test in `tests/bolt_v3_submit_admission.rs`
- [ ] T034 [US1] Run `quote_quantity_sell_stop_limit_helper_missing_quote_uses_submitted_quote_quantity` and confirm it fails before fallback implementation in `tests/bolt_v3_submit_admission.rs`
- [ ] T035 [US1] Implement the SELL StopLimit missing-quote-cache fallback in `src/bolt_v3_submit_admission.rs`
- [ ] T036 [US1] Add a failing `quote_quantity_sell_stop_limit_helper_missing_context_fails_closed` helper test in `tests/bolt_v3_submit_admission.rs` covering both missing instrument context and decimal parse failure
- [ ] T037 [US1] Run `quote_quantity_sell_stop_limit_helper_missing_context_fails_closed` and confirm it fails before fail-closed implementation in `tests/bolt_v3_submit_admission.rs`
- [ ] T038 [US1] Implement the SELL StopLimit missing-context fail-closed guard in `src/bolt_v3_submit_admission.rs`
- [ ] T039 [US1] Re-run SELL StopLimit missing-quote/missing-context strategy and helper regressions in `src/strategies/binary_oracle_edge_taker.rs` and `tests/bolt_v3_submit_admission.rs`
- [ ] T040 [US1] Add `quote_quantity_inverse_sell_limit_preserves_nt_notional` helper characterization coverage proving inverse Limit quote-quantity notional stays on the existing NT path in `tests/bolt_v3_submit_admission.rs`
- [ ] T041 [US1] Run `quote_quantity_inverse_sell_limit_preserves_nt_notional` before inverse bypass edits and record whether existing NT-path behavior is already preserved in the implementation handoff or PR body
- [ ] T042 [US1] Add `quote_quantity_inverse_sell_stop_limit_preserves_nt_notional` helper characterization coverage proving inverse StopLimit quote-quantity notional stays on the existing NT path in `tests/bolt_v3_submit_admission.rs`
- [ ] T043 [US1] Run `quote_quantity_inverse_sell_stop_limit_preserves_nt_notional` before inverse bypass edits and record whether existing NT-path behavior is already preserved in the implementation handoff or PR body
- [ ] T044 [US1] Add `quote_quantity_inverse_sell_limit_strategy_preserves_nt_notional` strategy regression proving inverse Limit strategy wiring stays on the existing NT path in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T045 [US1] Run `quote_quantity_inverse_sell_limit_strategy_preserves_nt_notional` before inverse bypass edits and record whether existing NT-path behavior is already preserved in the implementation handoff or PR body
- [ ] T046 [US1] Add `quote_quantity_inverse_sell_stop_limit_strategy_preserves_nt_notional` strategy regression proving inverse StopLimit strategy wiring stays on the existing NT path in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T047 [US1] Run `quote_quantity_inverse_sell_stop_limit_strategy_preserves_nt_notional` before inverse bypass edits and record whether existing NT-path behavior is already preserved in the implementation handoff or PR body
- [ ] T048 [US1] Preserve inverse-instrument bypass behavior for SELL Limit and StopLimit in `src/bolt_v3_submit_admission.rs` and `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T049 [US1] Run inverse helper and strategy regressions in `src/strategies/binary_oracle_edge_taker.rs` and `tests/bolt_v3_submit_admission.rs`
- [ ] T050 [US1] Run quote-quantity admission strategy/helper regressions plus existing BUY/Market admission regressions in `src/strategies/binary_oracle_edge_taker.rs` and `tests/bolt_v3_submit_admission.rs`
- [ ] T051 [US1] Add a deterministic source-fence test with a test-local positive control proving forbidden provider/market tokens fail and `src/bolt_v3_submit_admission.rs` passes in `tests/bolt_v3_submit_admission.rs`
- [ ] T052 [US1] Run the shared-helper source-fence positive-control and real-helper assertions in `tests/bolt_v3_submit_admission.rs`

**Checkpoint**: US1 passes without Polymarket, binary-oracle, market-family, or strategy identity in the generic helper.

---

## Phase 4: User Story 2 - Current Reachability Stays Explicit (Priority: P2)

**Goal**: Current supported paths stay unchanged, while unsupported shorts and quote-sized exits remain visibly blocked.

**Independent Test**: Existing quote-sized exit and short-position rejection tests still pass after the admission change.

### Tests for User Story 2

- [ ] T053 [US2] Re-run `exit_quote_quantity_config_is_blocked_before_base_position_quantity_is_used` in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T054 [US2] Re-run `exit_quote_quantity_order_build_is_rejected_before_nt_factory` in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T055 [US2] Re-run `configured_short_position_contract_is_rejected_until_short_economics_exists` in `src/strategies/binary_oracle_edge_taker.rs`
- [ ] T056 [US2] Re-run `quote_quantity_submit_admission_matches_nt_effective_notional_for_limit_buy`, `quote_quantity_submit_admission_uses_limit_price_when_nt_cache_quote_missing`, `quote_quantity_market_submit_admission_uses_nt_cache_quote_ask`, and `quote_quantity_market_submit_admission_uses_nt_cache_trade_when_quote_missing` in `src/strategies/binary_oracle_edge_taker.rs`

**Checkpoint**: US2 proves current reachability and non-reachability are explicit.

---

## Phase 5: Verification And Review

**Purpose**: Prove the final exact head before PR review.

- [ ] T057 Update `specs/452-quote-quantity-sell-admission/research.md` only for implementation evidence, not for #451 scope expansion
- [ ] T058 Run `cargo fmt -- --check`
- [ ] T059 Run targeted Rust tests for `tests/bolt_v3_submit_admission.rs`
- [ ] T060 Run targeted Rust tests for `src/strategies/binary_oracle_edge_taker.rs` quote-quantity and reachability cases
- [ ] T061 Run `cargo test --locked` and verify every `quickstart.md` `cargo test` filter matches at least one implemented test function
- [ ] T062 Run `just clippy`
- [ ] T063 Run the ai-slop-cleaner skill against the final diff before requesting review
- [ ] T064 Open a PR for issue #452 only
- [ ] T065 Confirm exact PR head CI is green
- [ ] T066 Request external exact-head review after all local checks pass and PR CI is green

---

## Dependencies & Execution Order

- Phase 1 blocks all runtime edits.
- Phase 2 blocks US1 and US2.
- US1 must complete before US2 preservation checks are trusted.
- Phase 5 starts only after US1 and US2 pass locally.
- No task implements #451 generic wrapper extraction.
