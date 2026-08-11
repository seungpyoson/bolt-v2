# Economics Slice 1 Review Repairs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the externally approved PR #1544 / issue #1445 repair design so every routed NT order has one purpose-typed economics basis, one provider fee authority, and one bounded cancellation coordinator for tracked maker orders.

**Architecture:** Keep `src/bolt_v3_order_execution.rs` as the shared routing facade, but move the two new state-heavy responsibilities into private `economics_basis` and `cancel_coordinator` submodules. Strategies supply typed value intent only; shared execution derives fills, gross value, lifecycle, admission purpose, clocks, retries, and NT operations. The approved design at `docs/superpowers/specs/2026-08-10-economics-slice-1-review-repairs-design.md` is the contract.

**Tech Stack:** Rust, NautilusTrader Rust API pinned by `Cargo.lock`, `rust_decimal`, TOML/Serde, existing Bolt economics/admission/evidence modules, Cargo/nextest, GitHub advisory CI.

## Global Constraints

- Scope is only PR #1544 / issue #1445 Slice 1 review repairs; do not create another issue, add live authority, or claim deploy/readiness/trading permission.
- Preserve `economics_slice = "quote_only"`; kill-switch cancellation remains proof-only.
- Keep one implementation per concern: one sealed economics constructor, provider adapters as the only fee authority, one tracked-maker cancellation coordinator, and one NT actor clock.
- Do not add compatibility constructors, raw gross/lifecycle inputs, source-scanning tests, code defaults, wall-clock fallbacks, or strategy-owned execution mechanics.
- Required config values live only in TOML. Use `cancel_retry_timeout_ms = 1000` and `cancel_recovery_escalation_attempts = 3` in every shipped economics section and fixture; Rust contains no fallback values.
- All local Cargo commands must use `CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs'`, `CARGO_BUILD_JOBS=2`, and test commands must append `-- --test-threads=1`. `/Volumes/T9` had 2.3 TiB free when this plan was written. If it is not mounted, stop before running Cargo.
- Prefer targeted local red/green checks. Exact-head full verification comes from advisory GitHub CI after a plain push; do not wait on CI.
- Every failure before final-basis construction or initial route validation leaves order evidence, exposure, counters, reservations, registrations, sink calls, and venue state unchanged.
- Every live pre-sink lifetime failure drops the uncommitted admission permit and registration guard, restoring counters and reservations without calling NT.
- Keep commits reviewable and use only behavior tests or compiler-enforced API deletion.

---

### Task 1: Required recovery configuration and cadence invariants

**Files:**
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_economics_runtime.rs`
- Modify: `src/bolt_v3_economics_test_support.rs`
- Modify: `src/strategies/binary_oracle_maker/mod.rs`
- Modify: `config/root.toml`
- Modify: `config/profiles/prod-btc-5m.overlay.toml`
- Modify: `tests/fixtures/bolt_v3/root.toml`
- Modify: `tests/fixtures/economics/hyperliquid/execution.toml`
- Modify: `tests/fixtures/legacy_prod_btc_5m_oracle.toml`
- Test: `tests/bolt_v3_economics_runtime.rs`
- Test: `tests/bolt_v3_binary_oracle_maker_runtime.rs`

**Interfaces:**
- Consumes: existing `ExecutionEconomicsConfig`, `BoundExecutionEconomics::config`, maker `quote_interval_ms`, and `resting_order_refresh_margin_ms`.
- Produces: required `NonZeroU64 cancel_retry_timeout_ms`, required `NonZeroU32 cancel_recovery_escalation_attempts`, `BoundExecutionEconomics::cancel_retry_timeout_ns()`, and `BoltV3OrderEconomicsHandle::validate_cancel_recovery_cadence(cadence_ns)`. The attempt-count accessor is added atomically with its first coordinator consumer in Task 5.

- [ ] **Step 1: Add failing config tests for missing, zero, overflow, and incompatible cadence**

Add behavior tests named:

```rust
fn execution_economics_requires_cancel_recovery_configuration()
fn execution_economics_rejects_zero_cancel_recovery_configuration()
fn maker_start_rejects_cancel_recovery_cadence_without_margin()
fn maker_start_accepts_bounded_cancel_recovery_cadence()
```

The tests must deserialize real fixture TOML, remove each required key independently, set each key to zero independently, exercise `u64` nanosecond overflow, and exercise both sides of:

```text
cadence_ns + ceil_to_cadence(retry_timeout_ns, cadence_ns)
    < resting_order_refresh_margin_ns
```

The rejection cases must fail before timer registration or market refresh.

- [ ] **Step 2: Run the focused tests and confirm the new cases fail**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test wiring_registration execution_economics_requires_cancel_recovery_configuration -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker maker_start_rejects_cancel_recovery_cadence_without_margin -- --test-threads=1
```

Expected: missing struct fields or assertions showing the unvalidated configuration is accepted.

- [ ] **Step 3: Add required typed fields and checked accessors**

Change the config shape to:

```rust
pub struct ExecutionEconomicsConfig {
    // existing required fields
    pub quote_validity_ms: u64,
    pub resting_order_refresh_margin_ms: u64,
    pub cancel_retry_timeout_ms: NonZeroU64,
    pub cancel_recovery_escalation_attempts: NonZeroU32,
    // existing authority maps
}
```

Add checked runtime accessors:

```rust
pub(crate) fn cancel_retry_timeout_ns(&self) -> Result<u64, EconomicsAdmissionError> {
    self.config
        .cancel_retry_timeout_ms
        .get()
        .checked_mul(NANOSECONDS_PER_MILLISECOND)
        .ok_or(EconomicsError::ArithmeticOverflow.into())
}

```

`ExecutionEconomicsConfig::validate_common` must reject `cancel_retry_timeout_ms >= resting_order_refresh_margin_ms`. `BoltV3OrderEconomicsHandle::validate_cancel_recovery_cadence` must use checked `div_ceil`, multiplication, and addition to prove the strict cadence inequality. Replace the maker timer's old cadence-only validation call with this method.

- [ ] **Step 4: Add the two required keys to every shipped economics section**

Add exactly:

```toml
cancel_retry_timeout_ms = 1000
cancel_recovery_escalation_attempts = 3
```

Place them beside `resting_order_refresh_margin_ms` in all five config/fixture files listed above. Update the economics test-support mutation table so its widened quote horizon retains these required values without inserting a fallback.

- [ ] **Step 5: Run the config and maker-start tests**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test wiring_registration execution_economics_requires_cancel_recovery_configuration -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker maker_start_rejects_cancel_recovery_cadence_without_margin -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker maker_start_accepts_bounded_cancel_recovery_cadence -- --test-threads=1
```

Expected: all focused tests pass.

- [ ] **Step 6: Commit the configuration contract**

```bash
git add src/bolt_v3_config.rs src/bolt_v3_economics_runtime.rs src/bolt_v3_economics_test_support.rs src/strategies/binary_oracle_maker/mod.rs config/root.toml config/profiles/prod-btc-5m.overlay.toml tests/fixtures/bolt_v3/root.toml tests/fixtures/economics/hyperliquid/execution.toml tests/fixtures/legacy_prod_btc_5m_oracle.toml tests/bolt_v3_economics_runtime.rs tests/bolt_v3_binary_oracle_maker_runtime.rs
git commit -m "feat(economics): require bounded cancel recovery"
```

### Task 2: Purpose-typed scenarios and the sealed final-order basis

**Files:**
- Create: `src/bolt_v3_order_execution/economics_basis.rs`
- Modify: `src/bolt_v3_order_execution.rs`

**Interfaces:**
- Consumes: final `OrderAny`, `InstrumentAny`, `BoltV3SubmitAdmissionRequestInput` facts without caller-selected intent kind, candidate `BoltV3PlannedFillLeg` levels, and existing provider economics/admission types.
- Produces: `BoltV3TerminalValueEntry`, `BoltV3FinalOrderEconomicsScenario`, `BoltV3FinalOrderEconomicsInput`, private `FinalOrderEconomicsBasis`, and a replacement `build_order_economics_submit_admission` that accepts no raw gross value, lifecycle, role, or intent kind.

- [ ] **Step 1: Write failing basis tests before defining the new types**

Create inline tests in `economics_basis.rs` with these behavior names:

```rust
fn base_quantity_seal_truncates_over_cover_and_recomputes_every_money_basis()
fn quote_quantity_seal_rolls_residual_into_a_later_cheaper_level()
fn quote_quantity_seal_rejects_candidate_undercoverage_instead_of_calling_it_dust()
fn quote_quantity_seal_accepts_a_price_less_market_order_without_inventing_a_limit()
fn final_basis_rejects_side_scenario_and_limit_mismatches()
fn planned_exit_gross_uses_the_post_clamp_quantity()
```

The quote residual case must use size increment `0.05`, quote quantity `1.23`, and descending prices where the first level leaves a residual that funds a later `0.05` base increment. It must assert retained legs, NT normalization, gross, planned-fill notional, provider fee inputs, final dust, full reservation liability, and order binding. The undercoverage case must assert construction fails with no admission or sink mutation.

- [ ] **Step 2: Run the new module test filter and confirm it fails to compile**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib quote_quantity_seal_rolls_residual_into_a_later_cheaper_level -- --test-threads=1
```

Expected: missing module/types or the old per-level consumption behavior fails the assertions.

- [ ] **Step 3: Define private-field scenario types that derive purpose**

Expose only fallible constructors and read-only accessors:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3TerminalValueEntry {
    expected_terminal_value_per_unit: Decimal,
    minimum_core_edge_ratio: Decimal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoltV3FinalOrderEconomicsScenario {
    TerminalValueEntry(BoltV3TerminalValueEntry),
    PlannedRiskReducingExit {
        stored_entry_cost_per_unit: Decimal,
        position: PositionContext,
    },
    ForcedReduction {
        position: PositionContext,
    },
}
```

The constructors must validate finite/positive unit values and valid position context. Methods on the enum must exhaustively derive:

```rust
fn intent_kind(&self) -> BoltV3SubmitIntentKind
fn lifecycle_path(&self) -> LifecyclePath
fn admission_policy(&self) -> EconomicsAdmissionPolicy
fn gross_expected_value(&self, legs: &[NautilusPlannedFillLeg]) -> Result<Decimal>
```

Do not expose a constructor parameter for lifecycle, submit intent, admission purpose, liquidity role, or absolute gross value.

- [ ] **Step 4: Replace the raw final input with a context that omits intent kind**

Define:

```rust
pub struct BoltV3FinalOrderEconomicsInput<'a> {
    pub execution_client_id: &'a str,
    pub intent: &'a OrderIntentDetails,
    pub order: &'a OrderAny,
    pub valuation: OrderValuationContext<'a>,
    pub risk_reducing_exit_position: Option<BoltV3RiskReducingExitPositionInput<'a>>,
    pub scenario: BoltV3FinalOrderEconomicsScenario,
    pub candidate_fill_levels: Vec<BoltV3PlannedFillLeg>,
    pub requested_at_ns: u64,
    pub decision_correlation_id: &'a str,
}
```

The private constructor creates `BoltV3SubmitAdmissionRequestInput` only after deriving `intent_kind` from `scenario`. Liquidity role comes only from `order.is_post_only()`.

- [ ] **Step 5: Implement exact-lattice allocation across the full execution ladder**

Implement one helper with this shape:

```rust
fn normalize_final_fill_levels(
    order: &OrderAny,
    instrument: Option<&InstrumentAny>,
    facts: BoltV3OrderAdmissionFacts,
    candidates: Vec<BoltV3PlannedFillLeg>,
) -> Result<NormalizedFinalFillPlan>
```

For quote-quantity orders, require an instrument and, in supplied order, perform:

```rust
let affordable_base = floor_to_size_increment(
    remaining_quote.checked_div(level.price).context("quote allocation division")?,
    instrument.size_increment(),
    instrument.size_precision(),
)?;
let retained_base = affordable_base.min(level.quantity);
let retained_notional = level
    .price
    .checked_mul(retained_base)
    .context("retained quote notional overflow")?;
remaining_quote = remaining_quote
    .checked_sub(retained_notional)
    .context("remaining quote subtraction")?;
```

Require both candidate and retained quantities to pass `Instrument::try_normalize_qty`. Continue after a zero retained level. Classify only the final residual as dust after every remaining-capacity level is unable to buy one increment. Separately calculate aggregate candidate notional and fail if it is below the submitted quote quantity. For base-quantity orders, consume base quantity directly and require zero residual.

- [ ] **Step 6: Build the sealed admission solely from retained levels**

`FinalOrderEconomicsBasis` must retain private derived values only:

```rust
struct FinalOrderEconomicsBasis {
    request: BoltV3SubmitAdmissionRequest,
    normalized_fill_legs: Vec<NautilusPlannedFillLeg>,
    planned_fill_notional: PlannedFillNotional,
    gross_expected_value: Decimal,
    reservation_basis: Decimal,
    order_binding: EconomicsOrderBinding,
    lifecycle_path: LifecyclePath,
    policy: EconomicsAdmissionPolicy,
}
```

Compute terminal-entry gross as `sum((terminal_value - price) * quantity)`, planned-exit gross as `sum((price - entry_cost) * quantity)`, and forced-reduction gross as zero. Call provider economics with the retained levels and build submit admission from the same final order binding. Delete `BoltV3OrderEconomicsIntent`, `BoltV3OrderEconomicsSubmitInput`, and the old `normalize_economics_fill_legs` path.

- [ ] **Step 7: Run all basis tests**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib economics_basis::tests -- --test-threads=1
```

Expected: all new base, quote, price-less, mismatch, and clamp tests pass.

#### Caller migration within Task 2: close the API cutover atomically

**Files:**
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/orders_admission.rs`
- Modify: `src/strategies/binary_oracle_maker/mod.rs`
- Modify: `tests/bolt_v3_binary_oracle_maker_runtime.rs`
- Modify: `tests/bolt_v3_economics_runtime.rs`

**Interfaces:**
- Consumes: the Task 2 scenario constructors and final seal defined above.
- Produces: all five caller-matrix rows using typed scenarios; no production caller can provide gross value, lifecycle, liquidity role, admission purpose, or intent kind independently.

- [ ] **Step 8: Add caller-matrix and fail-before-mutation tests**

Add behavior tests covering:

```rust
fn edge_candidate_and_final_entry_share_terminal_value_scenario()
fn edge_exit_seals_after_the_final_quantity_clamp()
fn maker_submit_derives_gross_from_terminal_value_and_final_order()
fn forced_reduction_derives_zero_gross_and_risk_reduction_purpose()
fn final_basis_failure_precedes_edge_evidence_exposure_and_admission_mutation()
fn maker_submit_without_terminal_value_fails_before_order_evidence()
```

Each success case must assert derived submit intent, lifecycle, liquidity role, admission purpose, gross value, retained legs, and final binding. Each failure case must assert zero evidence writes, unchanged exposure, zero admission counters, no registration, and no sink call.

- [ ] **Step 9: Run one entry and one maker test and confirm old construction fails the contract**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib edge_candidate_and_final_entry_share_terminal_value_scenario -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib maker_submit_derives_gross_from_terminal_value_and_final_order -- --test-threads=1
```

Expected: old raw constructors or duplicated gross calculations violate the new assertions.

- [ ] **Step 10: Migrate edge candidate sizing and final entry**

Delete `entry_gross_expected_value` and change `BoltV3TakerEconomicsSizingInput` to accept `BoltV3TerminalValueEntry`. Build that value once from the selected outcome's adjusted terminal probability and minimum edge ratio, then carry the same typed value into final sealing. Candidate sizing recomputes gross directly from candidate legs; final sealing recomputes it from retained final legs.

Move the final seal before `record_submit_linked_strategy_input_snapshot`, `PendingEntry` exposure mutation, submit-admission counters, or sink invocation. Preserve the existing cleanup behavior for later evidence/admission failures.

- [ ] **Step 11: Migrate the planned exit after the final clamp**

Construct `PlannedRiskReducingExit` only after `clamp_risk_reducing_exit_to_venue_position` and the final `OrderAny` quantity mutation. Supply stored entry cost per unit plus `PositionContext`; remove the strategy-side absolute exit gross calculation and the `LifecyclePath::PlannedExit` argument. Seal before changing exposure to `ExitPending`.

- [ ] **Step 12: Migrate maker and forced reduction**

Replace `BoltV3MakerOrderRoutingContext::gross_expected_value` with:

```rust
pub terminal_value_entry: BoltV3TerminalValueEntry,
```

For maker submits, pass the outcome fair terminal value per unit; cancel-only commands do not inspect the scenario. Delete `maker_command_gross_expected_value`. The kill-switch route constructs `ForcedReduction { position }`; it supplies neither zero gross nor lifecycle manually.

- [ ] **Step 13: Remove raw test construction and make API deletion compile-enforced**

Replace direct `BoltV3OrderEconomicsIntent` and raw submit input construction in `tests/bolt_v3_economics_runtime.rs` with the typed scenario/final input. Remove the raw structs and all imports. Do not add a source-text assertion; compilation is the proof that no caller remains.

- [ ] **Step 14: Run the caller behavior suites**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib edge_candidate_and_final_entry_share_terminal_value_scenario -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib edge_exit_seals_after_the_final_quantity_clamp -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib maker_submit_derives_gross_from_terminal_value_and_final_order -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib forced_reduction_derives_zero_gross_and_risk_reduction_purpose -- --test-threads=1
```

Expected: all caller-matrix and failure-ordering tests pass.

- [ ] **Step 15: Commit the sealed basis and every migrated caller together**

```bash
git add src/bolt_v3_order_execution.rs src/bolt_v3_order_execution/economics_basis.rs src/strategies/binary_oracle_edge_taker/mod.rs src/strategies/binary_oracle_edge_taker/tests/orders_admission.rs src/strategies/binary_oracle_maker/mod.rs tests/bolt_v3_binary_oracle_maker_runtime.rs tests/bolt_v3_economics_runtime.rs
git commit -m "refactor(economics): seal purpose typed order economics"
```

### Task 3: One actor clock, remaining-lifetime checks, and permit rollback

**Files:**
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_order_execution.rs`

**Interfaces:**
- Consumes: sealed `EconomicsAdmission::quote().valid_until_ns()`, configured refresh margin, existing admission permit `Drop` rollback, and NT `DataActor::clock()`.
- Produces: explicit-time `admit_with_economics_at`, explicit-time shadow evaluation, `BoltV3NtVenueMutationSink::actor_time_ns`, and two lifetime checks around admission.

- [ ] **Step 1: Add delayed-route and pre-sink rollback tests**

Add tests named:

```rust
fn total_lifetime_cannot_hide_insufficient_remaining_margin()
fn source_horizon_shorter_than_remaining_margin_fails_before_evidence()
fn exact_remaining_margin_boundary_is_accepted()
fn pre_sink_clock_advance_rolls_back_permit_and_registration()
fn production_economics_route_uses_only_injected_actor_time()
```

Use a test sink with a `VecDeque<u64>` of actor times. The rollback test must advance between route entry and the live sink boundary, then assert no sink call, restored counters/reservations, and no resting registration.

- [ ] **Step 2: Run the delayed-route test and confirm the wall-clock path fails it**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib total_lifetime_cannot_hide_insufficient_remaining_margin -- --test-threads=1
```

Expected: the old `current_unix_ns()` production entrypoint cannot reproduce the injected event-time boundary.

- [ ] **Step 3: Expose only explicit-time production admission methods**

Replace the production economics methods with:

```rust
pub(crate) fn admit_with_economics_at(
    &self,
    request: &BoltV3SubmitAdmissionRequest,
    economics: &EconomicsAdmission,
    now_ns: u64,
) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError>

pub(crate) fn evaluate_and_record_without_consuming_capacity_with_economics_at(
    &self,
    request: &BoltV3SubmitAdmissionRequest,
    economics: &EconomicsAdmission,
    now_ns: u64,
) -> Result<(), BoltV3SubmitAdmissionError>
```

The test-only raw admission helpers may keep their existing test clock behavior. No production economics caller may invoke `current_unix_ns()`.

- [ ] **Step 4: Make the NT sink the one actor-time boundary**

Extend the shared sink trait:

```rust
fn actor_time_ns(&self) -> Result<u64>;
```

The NT implementation returns `self.strategy.clock().get_time_ns().as_u64()`. Test sinks return the next injected value. Do not add a system-clock fallback.

- [ ] **Step 5: Reorder submit routing and add the second guard**

In `route_submit_with_sink`:

1. Read `route_now_ns` from the sink.
2. Validate execution authority, purpose, order binding, and `valid_until_ns - route_now_ns >= margin`.
3. Record valid intent evidence.
4. Admit using the same `route_now_ns`.
5. In Live, read `pre_sink_now_ns`, repeat the remaining-margin check, call NT, then commit the permit.

If step 5 fails, return while the permit and registration guard remain uncommitted so `Drop` restores all mutable admission state.

- [ ] **Step 6: Run explicit-time and rollback tests**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib total_lifetime_cannot_hide_insufficient_remaining_margin -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib pre_sink_clock_advance_rolls_back_permit_and_registration -- --test-threads=1
```

Expected: both pass with zero venue mutation in rejection cases.

- [ ] **Step 7: Commit the clock cutover**

```bash
git add src/bolt_v3_submit_admission.rs src/bolt_v3_order_execution.rs
git commit -m "fix(economics): bind admission to actor time"
```

### Task 4: Delete the obsolete market-family fee authority

**Files:**
- Modify: `src/bolt_v3_market_families/mod.rs`
- Modify: `src/bolt_v3_market_families/updown.rs`
- Modify: `src/bolt_v3_market_families/static_binary_event.rs`
- Modify: `src/bolt_v3_market_families/binary_outcome.rs`
- Test: existing tests in the four files above
- Test: `tests/bolt_v3_economics_runtime.rs`

**Interfaces:**
- Consumes: provider economics adapters already used by the sealed basis.
- Produces: provider adapters as the only fee-formula authority; settlement, quote-target, and unknown-family dispatch behavior remains unchanged.

- [ ] **Step 1: Strengthen non-fee behavior tests before deleting the seam**

Ensure the existing family tests separately assert:

```rust
fn settlement_payout_dispatches_by_registered_family()
fn maker_quote_targets_dispatch_by_registered_family()
fn unknown_family_dispatch_fails_closed()
```

Keep provider formula and replay-parity coverage in their existing provider/economics tests.

- [ ] **Step 2: Run the retained family behavior tests**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib settlement_payout_dispatches_by_registered_family -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib unknown_family_dispatch_fails_closed -- --test-threads=1
```

Expected: pass before deletion.

- [ ] **Step 3: Delete the family fee field, formulas, fallbacks, and lookup functions**

Remove:

```text
MarketFamilyValidationBinding::maker_binary_fee_curve
binary_outcome::maker_binary_fee_curve
updown::maker_binary_fee_curve
static_binary_event::maker_binary_fee_curve
unsupported_maker_binary_fee_curve
maker_binary_fee_curve_for_family
maker_binary_fee_curve_for_family_with_bindings
```

Remove only fee-specific fixture fields and assertions. Do not add a wrapper or replacement family lookup.

- [ ] **Step 4: Run retained family and provider economics tests**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib settlement_payout_dispatches_by_registered_family -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test wiring_registration final_nautilus_order_routes_through_its_exact_provider_authority -- --test-threads=1
```

Expected: retained behavior and provider economics tests pass.

- [ ] **Step 5: Commit the authority deletion**

```bash
git add src/bolt_v3_market_families/mod.rs src/bolt_v3_market_families/updown.rs src/bolt_v3_market_families/static_binary_event.rs src/bolt_v3_market_families/binary_outcome.rs tests/bolt_v3_economics_runtime.rs
git commit -m "refactor(economics): remove family fee authority"
```

### Task 5: Implement and route the exhaustive cancellation coordinator

**Files:**
- Create: `src/bolt_v3_order_execution/cancel_coordinator.rs`
- Modify: `src/bolt_v3_economics_runtime.rs`
- Modify: `src/bolt_v3_order_execution.rs`

**Interfaces:**
- Consumes: Task 1 retry configuration, immutable `OrderAny` query seed, current NT cache snapshots, and actor nanoseconds.
- Produces: `BoundExecutionEconomics::cancel_recovery_escalation_attempts()`, private `NtOrderQuerySeed`, `CancelRoutingState`, `CancelObservation`, `CancelOperation`, `RestingOrderCancelHealth`, `RestingOrderCancelRecord`, public read-only `BoltV3RestingOrderCancelHealthSnapshot`, exhaustive identity gate, exhaustive 4×6 transition function, and the only tracked-maker cancel/query routing path.

- [ ] **Step 1: Write table-driven tests for identity, observations, and all 24 transitions**

Add inline tests named:

```rust
fn venue_identity_gate_covers_capture_absence_equality_and_conflict()
fn venue_identity_conflict_is_a_monotonic_routing_hold()
fn venue_identity_conflict_holds_routing_without_bypassing_health_or_clock_checks()
fn order_status_partition_covers_every_pinned_nt_variant()
fn every_cancel_state_observation_pair_has_one_explicit_transition()
fn callbacks_cannot_overwrite_a_newer_attempt_generation()
fn retry_escalation_recoverability_conflict_and_liveness_compose()
fn coordinator_rejects_clock_regression_without_state_change()
```

The identity-conflict tests must start with non-default generation, deadline, counters, routing state, and health; observe a differing venue ID against retryable and terminal cache states; assert that `RecoveryIdentityConflict { captured, observed }` is added without changing routing state, generation, deadline, counters, seed, or pre-existing health; then assert later events cause no operation or retirement while actor-clock validation and due deadline health continue, a due sibling still progresses, and the aggregate error retains both failures.

- [ ] **Step 2: Run the 24-pair test and confirm the module is absent**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib every_cancel_state_observation_pair_has_one_explicit_transition -- --test-threads=1
```

Expected: missing module/types.

- [ ] **Step 3: Define the small closed enums and independent health facets**

Use these shapes:

```rust
enum CancelRoutingState {
    Ready,
    Attempting {
        generation: u64,
        operation: CancelOperationKind,
        not_before_ns: u64,
    },
    Backoff { not_before_ns: u64 },
    PendingCancel { not_before_ns: u64 },
}

enum CancelObservation {
    MissingUnqueryable,
    MissingQueryable,
    Retryable,
    PendingCancelUnqueryable,
    PendingCancelQueryable,
    Terminal,
}

#[derive(Default)]
struct RestingOrderCancelHealth {
    recovery_identity_unavailable: bool,
    recovery_identity_conflict: Option<BoltV3RecoveryIdentityConflict>,
    retry_escalated: bool,
    liveness: Option<BoltV3CancellationLivenessFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3RestingOrderCancelHealthSnapshot {
    client_order_id: ClientOrderId,
    recovery_identity_unavailable: bool,
    recovery_identity_conflict: Option<BoltV3RecoveryIdentityConflict>,
    retry_escalated: bool,
    liveness: Option<BoltV3CancellationLivenessFailure>,
}
```

Define public read-only `BoltV3RecoveryIdentityConflict` and `BoltV3CancellationLivenessFailure` value types with private conflict fields and accessors. Do not encode health as one replaceable enum. The separate fields are monotonic and can coexist. `BoltV3RestingOrderCancelHealthSnapshot` exposes read-only accessors, and `BoltV3OrderEconomicsHandle::resting_cancel_health()` returns snapshots sorted by client order ID for operator logging and behavior tests without exposing mutable coordinator state.

- [ ] **Step 4: Implement the identity-only query seed and pre-classification gate**

`NtOrderQuerySeed` privately owns an `OrderAny` clone but exposes only `as_query_order()`. Its transition function exhaustively matches:

```rust
match (captured_venue_id, cached_venue_id) {
    (None, None) => Unchanged,
    (None, Some(_)) => Captured,
    (Some(_), None) => Preserved,
    (Some(captured), Some(observed)) if captured == observed => Unchanged,
    (Some(captured), Some(observed)) => Conflict { captured, observed },
}
```

Conflict runs before routing status classification, adds the conflict facet/error, and returns a routing hold without changing routing state, generation, deadline, counters, seed, or pre-existing health. Once the facet exists, all later reconciliations keep the same routing hold while actor-clock observation and due monotonic deadline health continue.

- [ ] **Step 5: Implement exhaustive NT status classification and the 4×6 transition**

Match all 15 pinned `OrderStatus` variants without `_`. Classify positive-leaves partial fills as retryable and zero-leaves/closed orders as terminal. Implement one exhaustive `(CancelRoutingState, CancelObservation)` match containing the 24 approved cells. The transition returns only:

```rust
enum CancelTransition {
    NoOperation,
    Begin(CancelOperationKind),
    Remove,
}
```

No callback-facing function invokes NT.

- [ ] **Step 6: Implement generation, backoff, attempt counters, and health deadlines**

Add `BoundExecutionEconomics::cancel_recovery_escalation_attempts()` in this step, beside its first production consumer. `begin_operation` must checked-increment generation and operation counters, calculate `operation_not_before_ns = now_ns + retry_timeout_ns`, and enter `Attempting` before releasing ownership. `settle_operation` acts only if the same generation remains `Attempting`. Every cancel/query invocation, including synchronous failure, advances the same backoff. `SkippedByPolicy` advances nothing. At the exact configured count, add `retry_escalated`; at `quote_deadline_ns`, add the appropriate liveness facet without overwriting the other facets. The same drive returns an aggregate error after processing every sibling and leaves the typed snapshots available for subsequent inspection.

- [ ] **Step 7: Run the coordinator core tests before integration**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib cancel_coordinator::tests -- --test-threads=1
```

Expected: all identity, status, matrix, generation, and health tests pass in the working tree. Do not commit until the routing integration below consumes the coordinator and removes the old boolean path.

#### Routing integration within Task 5: remove the old cancellation path atomically

**Files:**
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `src/bolt_v3_quote_lifecycle.rs`
- Modify: `tests/bolt_v3_binary_oracle_maker_runtime.rs`

**Interfaces:**
- Consumes: the Task 5 coordinator plans defined above and the Task 3 actor-time sink.
- Produces: one tracked-maker registry with optional cancellation intent, NT-native query recovery, generation-safe cancel/query execution, exact cancel-all scope, and sibling-isolated aggregate errors.

- [ ] **Step 8: Add integration tests for intent creation, query recovery, re-entry, and scope**

Add behavior tests named:

```rust
fn healthy_resting_order_survives_timer_drives_without_a_cancel_intent()
fn every_cancel_origin_merges_into_one_tracked_intent()
fn missing_unqueryable_order_performs_no_nt_operation_and_becomes_loud()
fn captured_identity_routes_query_and_only_authoritative_cache_state_retires()
fn synchronous_pending_rejection_and_terminal_callbacks_cannot_duplicate_or_overwrite()
fn repeated_sync_failure_waits_until_the_exact_retry_boundary()
fn one_side_cancel_all_marks_only_matching_records_after_nt_accepts()
fn one_failing_record_does_not_starve_due_siblings()
fn partial_fill_retains_tracking_and_fill_void_recreates_cancel_only_tracking()
```

The query test must use the real `Strategy::query_order` boundary with a registered NT test strategy. Exercise missing venue ID, later `None -> Some` capture, query not-found/no-report, transport failure, restored open cache state, and terminal report. No test may assert source text.

- [ ] **Step 9: Run the healthy-order test and confirm the old timer model is inadequate**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker healthy_resting_order_survives_timer_drives_without_a_cancel_intent -- --test-threads=1
```

Expected: missing coordinator registry/API.

- [ ] **Step 10: Replace `cancel_pending` with one atomic tracked-order registry**

Keep one lock around records shaped as:

```rust
struct TrackedMakerOrderRecord {
    economics: Option<RestingOrderEconomicsRecord>,
    query_seed: NtOrderQuerySeed,
    cancellation: Option<RestingOrderCancelRecord>,
}
```

Healthy resting registrations have `cancellation: None`. Fill-void recovery may insert `economics: None` with one cancellation intent. Terminal reconciliation removes the entire record. Registration stores exact instrument, side, and the submitted `OrderAny` seed. Duplicate client IDs fail before sink mutation.

- [ ] **Step 11: Extend the NT sink with query and execute plans outside the lock**

Add:

```rust
fn query_order_via_nt(&mut self, seed: &OrderAny) -> Result<()>;
```

The production implementation calls only `Strategy::query_order(seed)`. Under the registry lock, reconcile current cache state and arm `Attempting`; release the lock; call cancel/query; reacquire; re-read cache; settle only the matching generation. Collect every primary error while continuing due sibling records, then return one aggregate error.

- [ ] **Step 12: Convert all tracked-maker origins**

Use one coordinator entrypoint for:

```text
economics refresh CancelRequired
quote-lifecycle cancel request
instrument/side cancel-all
strategy stop
externally observed pending cancel
running-state fill-void reopen
```

Keep `route_cancel_with_sink` for untracked transient edge-taker cancels, but make the coordinator its only caller for tracked maker orders. This is one mechanics boundary, not two retry authorities.

- [ ] **Step 13: Route cancel-all scope through the per-order coordinator**

Select exact `(instrument_id, order_side)` records and create or merge their cancellation intents. Do not call NT's scope-wide cancel API: it cannot exclude a matching record that the coordinator says is already pending or still in backoff. Fan out through the same per-order coordinator driver as every other tracked-maker origin, so each record independently chooses cancel, query, or no operation. On synchronous error, apply backoff only to that record and continue siblings. On `SkippedByPolicy`, create no intents or operations. Uncovered records remain eligible. Add a repeated-origin assertion proving that cancel-all cannot bypass an existing pending/backoff deadline.

- [ ] **Step 14: Remove quote-lifecycle retry authority**

Change `CancelRejected` handling so the leg retains its lifecycle state and emits no `LifecycleAction::Cancel`. Late-accept/orphan actions remain cancellation requests, but now enter the coordinator and inherit its deadline/backoff. Update the existing tests:

```rust
fn cancel_rejected_retains_requote_pending_without_routing()
fn cancel_rejected_retains_cancel_pending_without_routing()
fn cancel_rejected_without_an_outstanding_cancel_is_a_noop()
```

- [ ] **Step 15: Run coordinator integration and quote-lifecycle tests**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker healthy_resting_order_survives_timer_drives_without_a_cancel_intent -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker synchronous_pending_rejection_and_terminal_callbacks_cannot_duplicate_or_overwrite -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker one_side_cancel_all_marks_only_matching_records_after_nt_accepts -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib cancel_rejected_retains_cancel_pending_without_routing -- --test-threads=1
```

Expected: all focused tests pass; cancel/query counts remain bounded.

- [ ] **Step 16: Commit the coordinator and its only routing authority together**

```bash
git add src/bolt_v3_economics_runtime.rs src/bolt_v3_order_execution.rs src/bolt_v3_order_execution/cancel_coordinator.rs src/bolt_v3_quote_lifecycle.rs tests/bolt_v3_binary_oracle_maker_runtime.rs
git commit -m "refactor(maker): centralize tracked order cancellation"
```

### Task 6: Wire maker callbacks and real graceful draining

**Files:**
- Modify: `src/strategies/binary_oracle_maker/archetype.rs`
- Modify: `src/strategies/binary_oracle_maker/mod.rs`
- Modify: `tests/bolt_v3_binary_oracle_maker_runtime.rs`

**Interfaces:**
- Consumes: Task 5 tracked-order reconciliation and the pinned NT `Strategy::stop() -> bool` deferral contract.
- Produces: `MakerShutdownState`, seven NT order callbacks, deferred stop with no new quoting, and final `Component::stop(self)` only after authoritative terminal reconciliation.

- [ ] **Step 1: Add real lifecycle tests before adding the hooks**

Add behavior tests named:

```rust
fn maker_validation_rejects_manage_stop_true()
fn maker_stop_without_tracked_orders_proceeds_immediately()
fn maker_stop_defers_and_cancels_every_tracked_order()
fn draining_disables_quote_refresh_admission_and_submission()
fn draining_completes_only_after_the_last_authoritative_terminal_observation()
fn unresolved_query_or_identity_conflict_keeps_draining_running_and_loud()
fn running_fill_void_reopens_cancel_tracking_but_post_stop_claims_no_callback()
```

The main stop test must use NT's real `Strategy::stop`/`Trader` lifecycle under active quoting conditions. Assert timers and callbacks continue, zero quote/admission/submit calls occur after the stop request, and `Component::stop` runs only after all tracked records retire.

- [ ] **Step 2: Run the draining test and confirm current `on_stop` cannot satisfy it**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker draining_disables_quote_refresh_admission_and_submission -- --test-threads=1
```

Expected: current immediate teardown/cancel behavior violates deferred draining.

- [ ] **Step 3: Reject NT managed stop for this maker at both startup paths**

In `validate_strategy`, emit an error when `strategy.manage_stop` is true. In `raw_maker_config_from_config`, return an error for the same condition so direct preparation cannot bypass startup validation. Keep the shipped envelope value `manage_stop = false`.

- [ ] **Step 4: Add one maker shutdown state and one submission guard**

Add:

```rust
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum MakerShutdownState {
    #[default]
    Running,
    Draining,
}
```

One helper, `ensure_accepting_new_quotes`, gates public quote planning, reference quote planning, re-quote-capable market refresh, new economics admission, and maker submit commands. Cancellation commands and coordinator reconciliation remain allowed. While draining, return a typed loud error/skip and never create a new resting registration.

- [ ] **Step 5: Implement the seven NT callbacks as reconciliation triggers**

Populate the maker `nautilus_strategy!` block with:

```rust
fn on_order_pending_cancel(&mut self, event: OrderPendingCancel)
fn on_order_cancel_rejected(&mut self, event: OrderCancelRejected)
fn on_order_canceled(&mut self, event: &OrderCanceled)
fn on_order_filled(&mut self, event: &OrderFilled)
fn on_order_fill_voided(&mut self, event: &OrderFillVoided)
fn on_order_expired(&mut self, event: OrderExpired)
fn on_order_rejected(&mut self, event: OrderRejected)
```

Each passes only `client_order_id` and fresh `self.clock().get_time_ns().as_u64()` into the shared coordinator. The coordinator re-reads NT cache; event status/leaves never become authority.

- [ ] **Step 6: Implement deferred stop and move teardown to final `on_stop`**

Override `Strategy::stop() -> bool` in the macro block. Return `true` when no tracked records exist. Otherwise set `Draining`, merge a stop cancellation intent for every tracked record, and return `false`. During draining, the quote timer drives only coordinator reconciliation. When the final record is authoritatively removed, call public `Component::stop(self)` and return from the current callback/timer without further work.

`DataActor::on_stop` must only deregister the timer, unsubscribe, and deactivate runtime state. Remove the old stop-time direct cancel loop.

- [ ] **Step 7: Run the real maker lifecycle suite**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker maker_validation_rejects_manage_stop_true -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker maker_stop_defers_and_cancels_every_tracked_order -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker draining_disables_quote_refresh_admission_and_submission -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker unresolved_query_or_identity_conflict_keeps_draining_running_and_loud -- --test-threads=1
```

Expected: all lifecycle tests pass with no post-stop submission.

- [ ] **Step 8: Commit the graceful-stop wiring**

```bash
git add src/strategies/binary_oracle_maker/archetype.rs src/strategies/binary_oracle_maker/mod.rs tests/bolt_v3_binary_oracle_maker_runtime.rs
git commit -m "feat(maker): drain tracked orders before stop"
```

### Task 7: Full evidence, internal adversarial review, and exact-head publication

**Files:**
- Verify all files changed in Tasks 1–6
- Update: PR #1544 review record/comment only; keep the stable PR body free of transient SHA/CI status

**Interfaces:**
- Consumes: all Task 1–6 commits and the approved design.
- Produces: clean exact head, local targeted evidence from T9, advisory CI trigger, and a fresh required-review request.

- [ ] **Step 1: Run static hygiene checks before expensive verification**

Run:

```bash
git diff --check ac78f8fd5f5a133d1da69db7d8e34ffe17d44de6...HEAD
rg -n 'T[O]DO|T[B]D|F[I]XME|H[A]CK|fix[[:space:]]later|follow-[u]p' src tests config docs/superpowers/plans/2026-08-10-economics-slice-1-review-repairs.md
rg -n 'maker_binary_fee_curve|BoltV3OrderEconomicsIntent|BoltV3OrderEconomicsSubmitInput|cancel_pending: bool' src tests
git status --short
```

Expected: clean diff, no debt markers, no retired symbols, and no uncommitted files.

- [ ] **Step 2: Run formatting and the smallest complete local suites on T9**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo fmt --check
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib bolt_v3_order_execution -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test wiring_registration -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo clippy --locked --features test-current-evidence-inspection --lib --bins -- -D warnings
```

Expected: every command exits zero. Do not start another local Cargo command concurrently.

- [ ] **Step 3: Conduct an internal adversarial review against the approved design**

Inspect the full base-to-head diff and verify each design evidence row maps to a behavior test or compiler-enforced deletion. Specifically try to falsify:

```text
one final-order basis
later-level quote residual carry
candidate undercoverage vs final dust
purpose/lifecycle derivation
pre-mutation and pre-sink rollback
provider-only fee authority
all 24 cancellation transitions
venue-ID conflict hold
queryable/unqueryable recovery
re-entrant generation safety
cancel-all scope
drain suppression and completion
one actor clock
```

Repair any real finding, rerun the smallest affected test, and repeat this step until no substantive finding remains.

- [ ] **Step 4: Commit any verification-only adjustments and confirm a clean head**

If Step 3 changed files:

```bash
git add src/bolt_v3_config.rs src/bolt_v3_economics_runtime.rs src/bolt_v3_economics_test_support.rs src/bolt_v3_submit_admission.rs src/bolt_v3_order_execution.rs src/bolt_v3_order_execution/economics_basis.rs src/bolt_v3_order_execution/cancel_coordinator.rs src/bolt_v3_market_families/mod.rs src/bolt_v3_market_families/updown.rs src/bolt_v3_market_families/static_binary_event.rs src/bolt_v3_market_families/binary_outcome.rs src/bolt_v3_quote_lifecycle.rs src/strategies/binary_oracle_edge_taker/mod.rs src/strategies/binary_oracle_edge_taker/tests/orders_admission.rs src/strategies/binary_oracle_maker/archetype.rs src/strategies/binary_oracle_maker/mod.rs tests/bolt_v3_economics_runtime.rs tests/bolt_v3_binary_oracle_maker_runtime.rs config/root.toml config/profiles/prod-btc-5m.overlay.toml tests/fixtures/bolt_v3/root.toml tests/fixtures/economics/hyperliquid/execution.toml tests/fixtures/legacy_prod_btc_5m_oracle.toml
git commit -m "test(economics): close repair evidence"
```

Then run:

```bash
git status --short
git rev-parse HEAD
git diff --check ac78f8fd5f5a133d1da69db7d8e34ffe17d44de6...HEAD
```

Expected: clean status and clean diff check.

- [ ] **Step 5: Push the exact head and detach from CI waiting**

Run a plain push from `codex/1445-economics-cutover`:

```bash
git push
```

Record the printed head SHA. Do not wait for advisory CI; the push triggers the repository workflows.

- [ ] **Step 6: Request exact-head external and required native review**

Resolve GitHub node ID `U_kgDOEZMFhA` to its current login, confirm `.github/CODEOWNERS` still names that login, and request its native review on PR #1544. The review prompt must name the exact pushed head, base `ac78f8fd5f5a133d1da69db7d8e34ffe17d44de6`, pinned NT `e4167fd1ed5ce9db06b43a81417ab4096b8b84b6`, the approved design head `b22e7b213e3de41cec2ad66d8593dc09801c507d`, changed files, local T9 commands/results, and every invariant in Step 3.

Do not merge. Report the head SHA and that review/CI evidence is pending; merge remains blocked until the user explicitly authorizes it and the required reviewer approves that exact head.
