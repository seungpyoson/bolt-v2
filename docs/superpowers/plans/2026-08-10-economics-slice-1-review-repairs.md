# Economics Slice 1 Review Repairs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the finally approved PR #1544 / issue #1445 repair design so every routed NT order has one purpose-typed economics basis, one provider fee authority, and one bounded cancellation coordinator for tracked maker orders.

**Architecture:** Keep `src/bolt_v3_order_execution.rs` as the shared routing facade, with final-basis ownership in private `economics_basis` and cancellation ownership in private `cancel_coordinator`. The facade owns one typed submit-attempt result. The edge taker owns only its strategy-local, generation-checked exit exposure reducer and causal position fence; it does not own execution, fillability, normalization, admission, or sink classification. Strategies supply typed value intent only; shared execution derives fills, gross value, lifecycle, admission purpose, clocks, retries, and NT operations. After written approval, `docs/superpowers/specs/2026-08-10-economics-slice-1-review-repairs-design.md` is the contract. Its finding-to-repair table is the authoritative traceability map for Tasks 8A–12.

**Tech Stack:** Rust, NautilusTrader Rust API pinned by `Cargo.lock`, `rust_decimal`, TOML/Serde, existing Bolt economics/admission/evidence modules, Cargo/nextest, GitHub advisory CI.

## Global Constraints

- Scope is only PR #1544 / issue #1445 Slice 1 review repairs; do not create another issue, add live authority, or claim deploy/readiness/trading permission.
- Preserve `economics_slice = "quote_only"`; kill-switch cancellation remains proof-only.
- Keep one implementation per concern: one sealed economics constructor, provider adapters as the only fee authority, one tracked-maker cancellation coordinator, and one NT actor clock.
- Do not add compatibility constructors, raw gross/lifecycle inputs, source-scanning tests, code defaults, wall-clock fallbacks, or strategy-owned execution mechanics.
- Required config values live only in TOML. Use `cancel_retry_timeout_ms = 1000` and `cancel_recovery_escalation_attempts = 3` in every shipped economics section and fixture; Rust contains no fallback values.
- All local Cargo commands must use `CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs'`, `CARGO_BUILD_JOBS=2`, and test commands must append `-- --test-threads=1`. `/Volumes/T9` had 2.3 TiB free when this plan was written. If it is not mounted, stop before running Cargo.
- Prefer targeted local red/green checks. Exact-head full verification comes from advisory GitHub CI after a plain push; do not wait on CI.
- Every failure before final-basis construction or initial route validation leaves exposure, counters, reservations, registrations, sink calls, and venue state unchanged. It may append only the typed intent/preparation rejection evidence required by Task 9 or the forced-reduction rejection evidence required by Task 10.
- Every live pre-sink lifetime failure drops the uncommitted admission permit and registration guard, restoring counters and reservations without calling NT.
- Keep commits reviewable and use only behavior tests or compiler-enforced API deletion.

Tasks 1–6 below are the historical implementation record through reviewed head `4e0cd663a19c95ed0a6360660c070a12452134cb`; do not re-execute them. Only Tasks 8A–12 remain active after written-spec approval.

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

The shipped maker values are a discriminating positive case: `quote_interval_ms=1000`, `cancel_retry_timeout_ms=1000`, and `resting_order_refresh_margin_ms=5000` produce `1000 + ceil_to_cadence(1000, 1000) = 2000 < 5000`.

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
- Modify existing: `src/bolt_v3_order_execution/economics_basis.rs`
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
- Modify existing: `src/bolt_v3_order_execution/tracked_order_economics.rs`
- Modify existing: `src/bolt_v3_order_execution/tracked_order_economics/cancel_coordinator.rs`
- Modify: `src/bolt_v3_economics_runtime.rs`
- Modify: `src/bolt_v3_order_execution.rs`

**Interfaces:**
- Consumes: Task 1 retry configuration, immutable `OrderAny` query seed, current NT cache snapshots, and actor nanoseconds.
- Produces: a public re-exported `BoltV3OrderEconomicsHandle` whose fields and complete tracked-order aggregate are private to `tracked_order_economics`; `BoundExecutionEconomics::cancel_recovery_escalation_attempts()`; private `NtOrderQuerySeed`, `CancelRoutingState`, `CancelObservation`, `CancelEvent`, `CancelEffect`, `RestingOrderCancelHealth`, and `RestingOrderCancelRecord`; public read-only `BoltV3RestingOrderCancelHealthSnapshot`; exhaustive identity gate; exhaustive 4×6 transition function; one event-reducer state-mutation interface; and the only tracked-maker cancel/query effect runner.

- [ ] **Step 1: Write table-driven tests for identity, observations, and all 24 transitions**

Add inline tests named:

```rust
fn venue_identity_gate_covers_capture_absence_equality_and_conflict()
fn venue_identity_conflict_is_a_monotonic_routing_hold()
fn venue_identity_conflict_holds_routing_without_bypassing_health_or_clock_checks()
fn pending_cancel_identity_conflict_reports_stuck_pending_at_the_deadline()
fn order_status_partition_covers_every_pinned_nt_variant()
fn every_cancel_state_observation_pair_has_one_explicit_transition()
fn callbacks_cannot_overwrite_a_newer_attempt_generation()
fn retry_escalation_recoverability_conflict_and_liveness_compose()
fn coordinator_rejects_clock_regression_without_state_change()
```

The identity-conflict tests must start with non-default generation, deadline, counters, routing state, and health; observe a differing venue ID against retryable and terminal cache states; assert that `RecoveryIdentityConflict { captured, observed }` is added without changing routing state, generation, deadline, counters, seed, or pre-existing health; then assert later events cause no operation or retirement while actor-clock validation and due deadline health continue. A retained `PendingCancel` state must produce `StuckPendingCancel` at the deadline; other retained states produce `CancellationDeadlineExceeded` without trusting mismatched cached status.

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
    total_recovery_attempts: u32,
    recovery_identity_unavailable: bool,
    recovery_identity_conflict: Option<BoltV3RecoveryIdentityConflict>,
    retry_escalated: bool,
    liveness: Option<BoltV3CancellationLivenessFailure>,
}
```

Define public read-only `BoltV3RecoveryIdentityConflict` and `BoltV3CancellationLivenessFailure` value types with private conflict fields and accessors. Do not encode health as one replaceable enum. The separate fields are monotonic and can coexist. `BoltV3RestingOrderCancelHealthSnapshot` exposes read-only accessors for every field, including `total_recovery_attempts()`, plus one private `runtime_error()` derived only from the snapshot. A snapshot with no active facet returns `None`; otherwise its deterministic rendering includes the client order ID, total attempts, and every active facet in recoverability, conflict, escalation, liveness order. `BoltV3OrderEconomicsHandle::resting_cancel_health()` returns the same snapshots sorted by client order ID for operator logging and behavior tests without exposing mutable coordinator state.

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

Conflict runs before routing status classification, adds the conflict facet/error, and returns a routing hold without changing routing state, generation, deadline, counters, seed, or pre-existing health. Once the facet exists, all later reconciliations keep the same routing hold while actor-clock observation and due monotonic deadline health continue. Deadline classification uses only retained trusted routing state: `PendingCancel` maps to `StuckPendingCancel`, while every other state maps to `CancellationDeadlineExceeded`.

- [ ] **Step 5: Implement exhaustive NT status classification and the 4×6 transition**

Match all 15 pinned `OrderStatus` variants without `_`. Classify positive-leaves partial fills as retryable and zero-leaves/closed orders as terminal. Implement one exhaustive `(CancelRoutingState, CancelObservation)` match containing the 24 approved cells. The internal transition returns only:

```rust
enum CancelTransition {
    NoOperation,
    Begin(CancelOperationKind),
    Remove,
}
```

No callback-facing function invokes NT.

Wrap that transition in one private reducer:

```rust
fn apply_event(
    &mut self,
    seed: &mut NtOrderQuerySeed,
    event: CancelEvent<'_>,
) -> Result<CancelEffect>;
```

`CancelEvent` has only routing-capable timer observation, passive observation, matching-generation operation success, and matching-generation operation-unobserved variants. Callbacks and policy-suppressed timer drives both use the explicitly non-routing passive event; event names describe authority, not call-site provenance. Unobserved covers both synchronous NT routing failure and post-routing actor-clock/cache observation failure, and settles the already-armed generation before reporting. A matching operation-success event whose reconciliation fails must also settle its still-active generation to the armed backoff before returning the error. `CancelEffect` has only no-op, remove, cancel, and query variants. Delete separate plan, callback-reconcile, success-settle, and failure-settle methods. Tests and production integration use the same reducer interface; only the effect runner invokes NT.

Define `BoltV3OrderEconomicsHandle`, the registry lock/map, `TrackedMakerOrderRecord`, `RestingOrderEconomicsRecord`, private query seed, and optional coordinator record inside `tracked_order_economics.rs`. The parent `bolt_v3_order_execution.rs` re-exports the handle and health value types but cannot name or access any field or record type. A handle clone shares the same private registry. The parent receives semantic methods only and never receives a mutable record, mutable-registry callback, registry constructor, registration guard, or partial-record constructor.

Inside that aggregate, put the private query seed and optional coordinator record inside one `TrackedOrderCancellation` owner. Every cancellation origin calls its sole `request_intent(quote_deadline_ns)` method; no caller may construct or replace the optional coordinator record directly. Normal resting registration constructs an owner without an intent, while running-state fill-void reconciliation constructs the same owner and immediately requests one. Keep the cancellation reducer in `tracked_order_economics/cancel_coordinator.rs`; do not move economics registration or refresh into that reducer file. This makes generation/deadline/backoff preservation compiler-owned rather than a call-site convention without creating a cancellation monolith.

- [ ] **Step 6: Implement generation, backoff, attempt counters, and health deadlines**

Add `BoundExecutionEconomics::cancel_recovery_escalation_attempts()` in this step, beside its first production consumer. The timer event must checked-increment generation and operation counters, calculate `operation_not_before_ns = now_ns + retry_timeout_ns`, enter `Attempting`, and return one effect before releasing ownership. Operation-success and operation-unobserved events act only if the same generation remains `Attempting`; unobserved settles to the already-armed backoff. Every cancel/query effect, including a synchronous NT failure, advances the same backoff. `SkippedByPolicy` produces no timer event and advances nothing. At the exact configured count, add `retry_escalated`; at `quote_deadline_ns`, add the appropriate liveness facet without overwriting the other facets.

Delete `primary_error`. `RestingOrderCancelRecord::health_snapshot(client_order_id)` copies the checked total-attempt count and every health facet into the sole report type; only `BoltV3RestingOrderCancelHealthSnapshot::runtime_error()` decides whether and how to render an error. The effect runner calls it exactly once after each retained record reaches its final state for that iteration: after a no-op timer event, after an operation-unobserved event, or after an operation-success event and its cache re-read. Removed records emit no stale health. Callback events change state only; they never publish a competing error. The final aggregate preserves operation failures separately, processes every sibling, and contains at most one composed health entry per client order ID per drive.

Add these tests:

```rust
fn composed_cancel_health_snapshot_is_the_complete_runtime_report()
fn cancel_health_aggregate_reports_post_settlement_facets_once_and_processes_due_siblings()
fn synchronous_cancel_failure_settles_before_composed_health_collection()
```

The snapshot test uses a hand-written expected error string containing the client order ID, checked total attempts, and every active facet; it also asserts a healthy snapshot produces no error. The post-settlement integration test starts with coherent venue identity A, then makes the test sink replace the cached order with identity B during the cancel call. Settlement must discover the conflict at the quote deadline, and that same `drive_observed_resting_order_economics` result must contain the conflict and correct liveness exactly once while proving a due sibling operation occurred. The synchronous-failure test reaches escalation on the failed attempt, verifies backoff is settled before reporting, and asserts the composed health entry appears once beside the distinct sink failure. These tests fail against pre-operation-only health sampling, single-winner selection, and duplicate collection.

- [ ] **Step 7: Run the coordinator core tests before integration**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib cancel_coordinator::tests -- --test-threads=1
```

Expected: all identity, status, matrix, generation, and health tests pass in the working tree. Do not commit until the routing integration below consumes the coordinator and removes the old boolean path.

#### Routing integration within Task 5: replace the tracked-order aggregate atomically

**Files:**
- Modify: `src/bolt_v3_order_execution.rs`
- Modify existing: `src/bolt_v3_order_execution/tracked_order_economics.rs`
- Modify existing: `src/bolt_v3_order_execution/tracked_order_economics/cancel_coordinator.rs`
- Modify: `src/bolt_v3_quote_lifecycle.rs`
- Modify: `tests/bolt_v3_binary_oracle_maker_runtime.rs`

**Interfaces:**
- Consumes: the Task 5 coordinator plans defined above and the Task 3 actor-time sink.
- Produces: one privately owned tracked-maker economics handle and registry, optional cancellation intent, NT-native query recovery, generation-safe cancel/query execution, exact cancel-all scope, and sibling-isolated aggregate errors. `bolt_v3_order_execution.rs` contains no tracked record fields, registry access, cancellation constructors, or tracked-maker effect runner.

- [ ] **Step 8: Add integration tests for intent creation, query recovery, re-entry, and scope**

Add behavior tests named:

```rust
fn healthy_resting_order_survives_timer_drives_without_a_cancel_intent()
fn every_cancel_origin_merges_into_one_tracked_intent()
fn repeated_cancel_origins_preserve_the_first_deadline_and_backoff()
fn resting_submit_releases_the_registry_before_invoking_the_sink()
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

Expected: the current parent-owned registry permits direct record construction/replacement or the wished-for semantic API does not exist.

- [ ] **Step 10: Move the complete public handle and aggregate behind one module boundary**

Move `BoltV3OrderEconomicsHandle` itself into `tracked_order_economics.rs` and re-export it from `bolt_v3_order_execution.rs`. Keep one private lock around records shaped as:

```rust
struct TrackedMakerOrderRecord {
    economics: Option<RestingOrderEconomicsRecord>,
    cancellation: TrackedOrderCancellation,
}
```

The handle constructor is the only complete-aggregate constructor and initializes an empty private registry. Healthy resting registrations hold an owner with no intent. Fill-void recovery may insert `economics: None` through a semantic aggregate operation that creates one immediate intent. Terminal reconciliation removes the entire record. Registration stores exact instrument, side, and the submitted `OrderAny` seed. Duplicate client IDs fail before sink mutation.

Replace the parent-visible registration guard with one semantic `route_resting_submit` transaction. It validates and inserts the provisional record under the lock, releases the lock, invokes a `FnOnce() -> Result<BoltV3SubmitRoutingOutcome>` without handing it registry internals, leaves callback-reconciled state intact after `Submitted`, and reacquires the lock only to remove the provisional registration after suppression/error. The synchronous-reentry test makes the submit closure call `reconcile_tracked_order_at`; it must complete rather than block, proving no external sink or callback runs under the registry lock. Parent code never receives a guard or token.

Move every current direct `economics` and `tracked_orders` access behind semantic methods. Parent production and parent tests must not import `TrackedMakerOrderRecord`, `TrackedOrderCancellation`, `RestingOrderCancelRecord`, `NtOrderQuerySeed`, the registry map, or any registration guard/token. Tests that previously inserted or inspected records directly must drive registration, cancellation origins, callbacks, timers, and read-only health/ID snapshots through the same API as production.

- [ ] **Step 11: Extend the NT sink with query and execute plans outside the lock**

Add:

```rust
fn query_order_via_nt(&mut self, seed: &OrderAny) -> Result<()>;
```

The production implementation calls only `Strategy::query_order(seed)`. Under the registry lock, feed a timer event into the reducer and receive at most one effect; release the lock; execute only that effect; reacquire; re-read cache; feed the matching-generation operation result back into the same reducer. The runner never reads or mutates coordinator state directly. Collect one composed health error per affected record while continuing due siblings, then return one aggregate error containing every active facet.

The known regression `maker_submit_routes_through_shared_execution_policy_and_admission` must assert the canonical composed facet `recovery_identity_unavailable=true`, not the retired prose `identity unavailable`, while retaining the typed health assertion. Run this exact test red before changing the assertion and green afterward.

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

Select exact `(instrument_id, order_side)` records and create or merge their cancellation intents. Keep timer-owned all-orders reconciliation and cancellation-origin exact-observation reconciliation as distinct APIs; never overload an empty observation collection to mean all tracked orders. Do not call NT's scope-wide cancel API: it cannot exclude a matching record that the coordinator says is already pending or still in backoff. Fan out through the same per-order coordinator driver as every other tracked-maker origin, so each record independently chooses cancel, query, or no operation. On cache-read or synchronous routing error, keep the failure scoped to that record and continue siblings. On `SkippedByPolicy`, create no intents or operations. Uncovered records remain eligible. Add assertions proving zero matches and selected-record cache failures never refresh or cancel uncovered records, and a repeated origin cannot bypass an existing pending/backoff deadline.

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
git add src/bolt_v3_economics_runtime.rs src/bolt_v3_order_execution.rs src/bolt_v3_order_execution/tracked_order_economics.rs src/bolt_v3_order_execution/tracked_order_economics/cancel_coordinator.rs src/bolt_v3_quote_lifecycle.rs tests/bolt_v3_binary_oracle_maker_runtime.rs
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

### Task 7: Historical checkpoint — superseded, do not execute

Tasks 1–6 produced reviewed head `4e0cd663a19c95ed0a6360660c070a12452134cb`. The former publication and review steps in this task are retired because Tasks 8–11 move the head. Task 12 is the only remaining publication and review gate. No sequential executor may push or request review from this checkpoint.

## Post-review systematic repair addendum

### Task 8A: Put the neutral core under root-workspace verification

**Files:**
- Modify: `Cargo.toml`, `.gitignore`, `justfile`, `.github/workflows/advisory.yml`
- Delete: `crates/economics-core/Cargo.lock`
- Preserve unchanged: `crates/backtesting-vertical-slice/Cargo.toml`, `crates/backtesting-vertical-slice/Cargo.lock`, and its isolated verification commands

**Interfaces:**
- Produces a root workspace containing only `bolt-v2` and `economics-core`.
- Preserves the independent backtesting workspace so its cloud/backtest features never unify into LiveNode.

- [ ] Add a root `[workspace]` with `crates/economics-core` as a member and `crates/backtesting-vertical-slice` explicitly excluded.
- [ ] Delete the ignored core lockfile and its `.gitignore` rule. Assert that no `crates/economics-core/Cargo.lock` remains, while the backtesting lockfile remains present.
- [ ] Put `--workspace`/`--all` on fmt, Clippy, and nextest in both the `justfile` and advisory workflow. Keep the BTE recipes in its own workspace with its own `--locked` lockfile.
- [ ] Verify root `cargo metadata --locked` lists `bolt-v2` and `bolt-economics-core` in the root workspace and excludes `backtesting-vertical-slice`.
- [ ] Run workspace fmt, core unit/synthetic-extension tests, and workspace Clippy on T9, sequentially with `CARGO_BUILD_JOBS=2`.
- [ ] Commit this workspace-governance boundary separately from cancellation cleanup.

### Task 8B: Delete retired cancellation authorities

**Files:**
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `src/bolt_v3_order_execution/tracked_order_economics.rs`
- Modify: `src/bolt_v3_quote_lifecycle.rs`

**Interfaces:**
- Leaves tracked maker per-order coordinator fan-out as the only production cancel-all authority.
- Leaves the private fail-closed modify sink because maker routing consumes it.

- [ ] Before changing this boundary, run the existing graceful-stop/coordinator tests on T9. Add a real `Trader` stop-deferral integration test if the current test calls `Strategy::stop` directly rather than exercising Trader's registered stop closure; prove timers/callbacks remain available while deferred.
- [ ] Delete the uncalled public `route_cancel_all` and `route_modify` wrappers.
- [ ] Delete the caller-less public `BoltV3NtOrderManagementContract` and `nt_order_management_contract()` census plus imports used only by that census; retired batch-cancel types must not remain advertised through a public dead-code escape hatch.
- [ ] Delete `route_cancel_all_with_sink`, `BoltV3CancelAllRoutingOutcome`, the batch-cancel sink trait methods/implementations, and the differential-only batch test. The tracked shadow branch returns its typed policy skip directly; it does not retain a test-only production sink.
- [ ] Delete the test compatibility alias and rename all exact-observation calls to `drive_observed_resting_order_economics`.
- [ ] Update the four quote-lifecycle comments to describe coordinator-scoped per-order fan-out.
- [ ] Run focused order-execution, cancellation, and maker graceful-stop tests on T9.
- [ ] Commit this cancellation-authority boundary separately.

### Task 9: Compile executable partial reductions and make exit evidence truthful

**Files:**
- Modify existing: `src/bolt_v3_executable_cost.rs`
- Modify existing: `src/bolt_v3_order_execution.rs`, `src/bolt_v3_order_execution/economics_basis.rs`, `src/bolt_v3_order_execution/tracked_order_economics.rs`, `src/bolt_v3_submit_admission.rs`
- Modify existing: `src/strategies/binary_oracle_edge_taker/mod.rs`, `config.rs`, `exposure.rs`
- Modify: `src/bolt_v3_current_evidence/facts.rs`, `codec/exit.rs`, `codec.rs`, `record.rs`, `handles.rs`, `reader.rs`, `generated_contract.rs`
- Modify: `config/decision-evidence-contract.toml` and current-evidence fixtures
- Test: edge-taker order/admission/evidence/adverse-path suites and evidence codec/round-trip suites

**Interfaces:**
- Produces low-level `compile_bounded_risk_reducing_ioc` plus one shared `compile_and_seal_risk_reducing_ioc` choke point. The latter consumes the requested `OrderAny`/typed intent, canonical NT position, authoritative book, configured depth, shared venue/instrument normalization, and one validated market-IOC template; it returns the final order, retained fills, intent, and sealed economics as one typed result.
- Produces a final quantity already accepted by shared execution and retained fill legs whose sum equals that quantity exactly; no strategy or later clamp can mutate one without rebuilding the whole result.
- Replaces `Result<BoltV3SubmitRoutingOutcome>` with one exhaustive shared `BoltV3SubmitAttemptOutcome` that classifies route validation, intent-evidence rejection, admission rejection, policy skip, pre-sink rejection, sink rejection, and submission at their source. Every submit caller migrates; there is no compatibility result path or caller-side string/downcast classification.
- Produces typed `Intent -> PreparedOrder -> AttemptOutcome` evidence instead of a transient `submitted` boolean, and one generation-checked exposure reducer in which only `Submitted` commits `ExitPending`.

- [ ] Add failing compiler cases for full depth, thin depth, sub-increment coverage, below-minimum coverage, zero-after-alignment, and fill-leg sum equality. Assert exact-entry pricing is unchanged.
- [ ] Add one complete config predicate for `Market + IOC + base_quantity + !post_only` with no trigger/trailing fields. Pass `is_reduce_only` through but do not use it as risk proof; typed intent plus the canonical-position clamp own that invariant. Reject every other non-post-only exit template at load time.
- [ ] Implement the shared choke point in order execution: canonical-position clamp -> bounded book compilation -> shared venue/instrument normalization/minimum check -> final order rewrite -> exact fill derivation -> economics seal. Reject a later canonical-position mismatch instead of silently reclamping.
- [ ] Add a failing submit-path test: a ten-unit position with five executable units submits and seals exactly five.
- [ ] Add the complete residual event sequences:
  - reduced-size five-unit IOC fills all five, the typed pending-exit state reconciles from the authoritative position cache/event/timer, the five-unit residual becomes `Managed`, and a later evaluation actually routes another reduction;
  - the compiled IOC is itself partially filled and then canceled/expired, the larger residual becomes `Managed`, and a later evaluation routes another reduction.
- [ ] Implement a typed `PositionReductionFence` in `TerminalFillAwaitingPosition` carrying the position/order identity, compiled and cumulatively filled quantities, terminal trade/event identity and timestamp, and a pre-submit canonical position stamp containing quantity, `ts_last`, and last-event identity. Checked arithmetic derives the maximum residual. Cache absence, generic timestamp advance, or smaller quantity alone is not terminal proof. Release only when the cached position contains the terminal trade ID or its last position event is explicitly reconciliation-marked, at/after the terminal fill, distinct from the captured event stamp, and within the maximum-residual bound; a cached closed position must satisfy the same causal proof.
- [ ] Run the terminal-fill sequence with position-before-fill and fill-before-position event ordering. Add the pinned-NT reconciliation case where `project_reconciliation_fill(..., apply_position=false)` makes the order terminal while position quantity/stamp remain unchanged: an immediate timer must remain awaiting and route nothing; unrelated later position activity must also remain awaiting; after a causally newer reconciliation-marked position cache update with no callback, the next timer remanages exactly five and the next evaluation routes only five. Duplicate reconciliation is idempotent.
- [ ] Add a canonical-position race test: if the position shrinks after compilation but before sealing/routing, reject the attempt with no second clamp, no evidence mismatch, and no admission/sink mutation.
- [ ] Add zero-depth/preparation-failure evidence proving no exposure, admission, or sink mutation.
- [ ] Replace the overloaded exit evidence with pre-preparation `ExitIntentDecisionFact`, prepared-only `ExitPreparedOrderFact`, and exhaustive `ExitAttemptOutcome` on `ExitEvaluationFact`. `PreparationRejected` carries a typed stage/reason and no prepared/submitted linkage. Route, intent-evidence, admission, policy, pre-sink, and sink outcomes carry `PreparedOrderLinkage`; only `Submitted` carries the distinct `SubmittedOrderLinkage`. Do not add a redundant standalone preparation-result fact.
- [ ] Replace the shared submit `anyhow::Result` boundary with the exhaustive typed outcome and map each failure where it originates. Inject every route phase independently and assert its exact typed outcome, linkage kind, counter/reservation rollback, sink count, and evidence; no string parsing or downcasts are allowed.
- [ ] Migrate resting-submit registration to the same typed outcome, superseding the historical Task 5 closure signature: retain the provisional tracked record only on `Submitted`; remove it on every other variant without erasing that variant. Reentrant callback state remains generation-authoritative.
- [ ] Add an exhaustive generation-checked exit-attempt reducer: arm `ExitAttempting` before routing; every non-submitted outcome returns the same generation to `Managed`; only `Submitted` commits `ExitPending`; a synchronous callback that advances state cannot be overwritten by the route return. Replace the current shadow latch test with `PolicySkipped` evidence, zero sink/capacity mutation, `Managed` exposure, and a later eligible exit attempt.
- [ ] Update codecs, generated contract, fixtures, census/contract entries, and round-trip tests atomically. Assert a requested quantity of ten can never be encoded as the submitted quantity when compilation produced five, and a prepared-but-skipped client order ID can never be decoded as a submitted-order linkage.
- [ ] Implement `TerminalFillAwaitingPosition` and its causal fence as an exhaustive enum state resolved by one cache/position-event/timer reducer; do not add a boolean latch or an unfenced "cache is current" conditional.
- [ ] Run focused edge-taker compiler, admission, evidence, codec, and adverse-path tests on T9.
- [ ] Commit compiler/state-machine and evidence-schema changes as separately reviewable cohesive commits.

### Task 10: Make root/runtime/provider authority honest

**Files:**
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_validate.rs`, `src/bolt_v3_validate/clients.rs`, `src/bolt_v3_validate/risk.rs`, `src/bolt_v3_validate/kill_switch.rs`
- Modify: `src/bolt_v3_providers/mod.rs`, provider execution/economics modules
- Modify: `src/bolt_v3_order_execution.rs`, `src/bolt_v3_submit_admission.rs`
- Modify: current-evidence admission facts/codecs/contracts/fixtures as required for a typed seal rejection
- Modify: `src/bolt_v3_live_node/risk_admission_loss.rs` only to consume the shared typed resolver; add no mirrored predicate or message
- Modify: shipped config and economics fixtures that own removed fields/components
- Test: root config, shared forced-reduction router, provider economics, and kill-switch suites

**Interfaces:**
- Produces one loaded-config economics/flatten resolver consumed by loaded-config validation and live-node construction.
- Produces one root-aware provider-economics validation path for every configured client.
- Produces one shared forced-reduction pre-admission rejection recorder.

- [ ] Replace the duplicated flatten prerequisite checks/messages with one pure typed resolver over `LoadedBoltV3Config`; call it after root/strategy loading and from live-node construction. Keep `validate_root_only` limited to block-local checks. Add the quote-only-economics incompatibility to the resolver rather than a third conditional block.
- [ ] Confirm every shipped config keeps `flatten_open_positions_on_breach=false`; if any selected shipped config differs, correct it atomically and disclose the lasting behavior.
- [ ] In `validate_clients_block(root)`, load each provider economics block through the registry and call `ExecutionEconomicsConfig::validate_common` with root reporting configuration before runtime binding, including unselected clients. Provider-local validation continues to own only provider fields.
- [ ] Extend forced-reduction rejection evidence with a typed `EconomicsSealRejected` reason. In the shared flatten router, record exactly one rejected `ForcedReductionAdmissionFact` when sealing fails, then return; assert zero admission counters/reservations/sink calls. Callers do not mirror this path.
- [ ] Remove Polymarket `mbf`/`tbf` builder-charge construction and its config/fixture component; retain and numerically pin authoritative platform-fee behavior.
- [ ] Validate the Polymarket rounding/sub-quantum pair and full economics config during root load.
- [ ] Delete dead `fee_cache_ttl_secs` and Hyperliquid aligned-product policy fields from schema and every shipped fixture.
- [ ] Reject, rather than waive, an attached Hyperliquid builder charge for unsupported spot buys.
- [ ] Run focused root-config, provider, shared-router, admission-evidence, and kill-switch tests on T9.
- [ ] Commit root/runtime authority and provider-formula cleanup as separate cohesive commits.

### Task 11: Close neutral-core and evidence contract gaps

**Files:**
- Modify: `crates/economics-core/src/edge.rs`, `quote.rs`, `types.rs`, `lib.rs`
- Modify: `src/bolt_v3_economics_runtime.rs`, `src/shadow_pnl.rs`
- Modify: maker economics scenario/policy call sites and tests
- Modify: `src/strategies/binary_oracle_edge_taker/entry_decision.rs`
- Modify: affected root/backtesting adapters and tests

**Interfaces:**
- Produces a currency-typed gross edge input, typed maker breakeven policy, and admission-authoritative resting equivalence.

- [ ] Add failing core tests for gross-currency mismatch, missing guaranteed point valuation, and non-positive position quantity.
- [ ] Introduce the typed gross amount, migrate every root/replay caller, and delete duplicate/unused core APIs.
- [ ] Replace the maker's raw zero floor with a typed breakeven policy. Prove a negative terminal-value gross is rejected and a positive value is admitted through the same policy.
- [ ] Add paired resting-refresh evidence: forecast-only drift does not cancel, while independently changing core quote, core edge, binding, and reservation terms still fails equivalence and produces the existing fail-closed refresh outcome.
- [ ] Preserve refreshed forecast fields in the stored admission and return a typed forecast-drift diagnostic without admitting or de-authorizing the order.
- [ ] Add a shadow-PnL test that strips bound economics from an admitted entry and asserts the loud error.
- [ ] Correct the entry-state comment and invalid backtesting manifest fixture versions without changing the isolated BTE workspace.
- [ ] Run core, economics-runtime, maker, shadow-PnL, and affected backtesting tests on T9.

### Task 12: The only exact-head closure and review gate

- [ ] Confirm by direct plan inspection that Tasks 1–11 contain no active push, publication, or review request; Task 7 remains historical/inert and Task 12 is the sole authority.
- [ ] Before closure, update the stable PR body only for lasting behavior: executable partial risk reductions, truthful preparation evidence, and startup rejection of automatic flattening under quote-only economics. Do not add transient SHA/CI status.
- [ ] Resolve the live PR base immediately before verification (`gh pr view 1544`); expected current base is `e62584045629208e81d2dce1fce608720ea01fbf`, prior reviewed head is `4e0cd663a19c95ed0a6360660c070a12452134cb`, and the repair delta is `4e0cd663...<new-head>`. Do not reuse the historical `ac78f8fd` anchor.
- [ ] Bind the review request to the exact commit containing the finally approved design (resolve its SHA after this design review), pinned NT `e4167fd1ed5ce9db06b43a81417ab4096b8b84b6`, the exact pushed code head, and the live PR base.
- [ ] Run `cargo fmt --all -- --check`, workspace Clippy with warnings denied, workspace nextest, and the root build using `CARGO_TARGET_DIR=/Volumes/T9/bolt-v2-target-1544-review-repairs` and `CARGO_BUILD_JOBS=2`; never run Cargo concurrently. Run BTE's own locked verification from its isolated workspace separately.
- [ ] Run `git diff --check <live-base>...HEAD`, inspect removed-symbol call graphs, verify the finding-to-repair map, and conduct an internal adversarial review of the full base-to-head diff. Repair findings before publication.
- [ ] Commit by cohesive boundary, confirm a clean worktree, push the exact head, and report it without waiting on advisory CI.
- [ ] In the review request, require advisory evidence to show the root workspace and core test targets. Push and report without waiting; the reviewer verifies that evidence at the new head.
- [ ] Request fresh external and native review only after all local findings are resolved, every applicable review comment is answered, and the exact head is pushed. Task 12 is the only review request in this plan.
