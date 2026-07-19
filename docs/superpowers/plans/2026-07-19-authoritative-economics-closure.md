# Authoritative Economics Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish PR #1446 by making each economics fact have one typed meaning, one producer, and one sealed downstream authority across production, shadow, replay, capital, and baskets.

**Architecture:** Keep the existing venue-neutral `src/economics/` evaluator and venue adapters. Replace raw notional scalars and downstream recalculation with typed authoritative quantities, move policy invariants into the one evaluator, and make capital/baskets consume only the sealed full liability. Production and replay differ only in capture acquisition.

**Tech Stack:** Rust, `rust_decimal`, NautilusTrader Rust API, TOML configuration, nextest remote archives, Python source fences.

## Global Constraints

- No runtime hardcodes, venue-name branches in shared code, conditional fallbacks, alternate authorities, or compatibility paths.
- Provider behavior is accepted only when proved by governed fixtures and configured formula policy; unsupported shapes reject.
- Strategies produce intent and planned execution facts only; shared execution/admission owns economics and reservation.
- Live submission remains disabled.
- The frozen design commit remains detached and read-only.
- Rust behavior verification is remote-first. Each RED and GREEN head is committed and published for governed exact-head execution.

---

### Task 1: Give Every Monetary Meaning a Distinct Type

**Files:**
- Modify: `src/economics/types.rs`
- Modify: `src/bolt_v3_economics_runtime.rs`
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Test: `tests/bolt_v3_economics_runtime.rs`
- Test: `tests/bolt_v3_submit_admission.rs`

**Interfaces:**
- Produces: `PlannedFillNotional`, `ReservationBasis`, `GuaranteedDebit`, `FullReservationLiability`, and `EdgeBasisAmount` validated newtypes.
- Produces: `EconomicsAdmission::{planned_fill_notional,reservation_basis,guaranteed_debit,full_reservation_liability}`.
- Removes: raw `base_reservation_notional`, `reservation_notional`, and copied submit-request `notional` authority.

- [ ] **Step 1: Write failing semantic-separation tests**

Add tests proving a multi-level market exit can have unequal planned fill notional and reservation basis, while full liability equals the checked sum of reservation basis and guaranteed debit. Add a compile-backed contract proving downstream admission reads `full_reservation_liability()` rather than a copied `Decimal`.

```rust
assert_ne!(admission.planned_fill_notional().amount(), admission.reservation_basis().amount());
assert_eq!(
    admission.full_reservation_liability().amount(),
    admission.reservation_basis().amount() + admission.guaranteed_debit().amount(),
);
```

- [ ] **Step 2: Publish the tests-only head and verify RED**

Run local non-compile gates, commit the tests, publish with `just sandbox-safe-push`, and run the governed exact-head remote suite. Expected: compile failure because the typed API does not exist, proving the tests exercise the intended cutover.

- [ ] **Step 3: Add validated zero-cost newtypes**

Implement private-field newtypes in `src/economics/types.rs`. Constructors reject negative values; `PlannedFillNotional` and `EdgeBasisAmount` reject zero. Only explicit checked operations may combine compatible meanings.

```rust
pub struct FullReservationLiability(Decimal);

impl FullReservationLiability {
    pub fn try_from_parts(
        basis: ReservationBasis,
        debit: GuaranteedDebit,
    ) -> Result<Self, EconomicsUnavailable> { /* checked add */ }

    pub fn amount(self) -> Decimal { self.0 }
}
```

- [ ] **Step 4: Cut the admission seal over atomically**

Derive `PlannedFillNotional` once from canonical planned legs, accept `ReservationBasis` from the final order mapper, derive `GuaranteedDebit` from the core quote, and seal `FullReservationLiability`. Delete cross-meaning equality gates and copied scalar fields; keep order-binding checks over canonical order facts.

- [ ] **Step 5: Verify GREEN remotely and commit**

Expected: targeted economics runtime and submit-admission tests pass; existing expiry, binding, and negative-edge tests remain green.

### Task 2: Put Every Invariant Inside One Evaluator

**Files:**
- Modify: `src/bolt_v3_economics_runtime.rs`
- Modify: `crates/backtesting-vertical-slice/src/economics.rs`
- Test: `tests/bolt_v3_economics_runtime.rs`
- Test: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_economics.rs`

**Interfaces:**
- Consumes: typed quantities from Task 1.
- Produces: one `ConfiguredEconomicsAdmissionSource` policy path used by production and replay.
- Removes: replay-owned quote-validity and admission-policy derivation.

- [ ] **Step 1: Write failing production/replay equivalence tests**

Use the same request, captured provider payloads, configuration, and clock in both modes. Assert identical sealed admissions and identical typed failures at refresh, maximum-age, quote-validity, and resting-margin boundaries.

- [ ] **Step 2: Verify RED remotely**

Expected: replay differs on at least the edge-basis/freshness policy boundary.

- [ ] **Step 3: Move policy checks into the shared admission evaluator**

Make replay parse the same economics policy from snapshot-carried TOML. Retain replay-only capture-integrity checks, but delete replay-specific economics validation and validity construction.

- [ ] **Step 4: Verify GREEN remotely and commit**

Expected: byte-equivalent admissions for equal facts and equal typed rejection for mutated timestamps/configuration.

### Task 3: Make the Seal the Sole Capital and Basket Authority

**Files:**
- Modify: `src/bolt_v3_capital_admission.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_basket_admission.rs`
- Modify: `src/bolt_v3_outcome_group_scanner.rs`
- Test: `tests/bolt_v3_submit_admission.rs`
- Test: `tests/bolt_v3_basket_admission.rs`

**Interfaces:**
- Consumes: `FullReservationLiability` from Task 1.
- Capital owns only availability, caps, reserve, commit, and rollback.
- Basket owns identity, atomicity, and checked sum of sealed liabilities.
- Scanner retains selection/executable-price evidence but produces no reservation authority.

- [ ] **Step 1: Write failing outer-boundary tests**

Add a basket whose base values fit the cap but sealed full liabilities exceed it. Assert rejection before mutation. Add the below-cap twin and assert the stored reservation equals the checked sum of sealed liabilities. Add a production-shaped capital request that needs no parallel admission evidence.

- [ ] **Step 2: Verify RED remotely**

Expected: current outer basket admits or stores scanner cost; production-shaped capital request rejects with missing parallel evidence.

- [ ] **Step 3: Delete parallel arithmetic and evidence**

Capital consumes the typed sealed liability directly and performs no price×quantity economics calculation. Basket uses checked sum of per-leg sealed liabilities for both cap and reservation. Remove scanner `total_adjusted_cost`, edge, and slippage fields only where they serve admission/reservation authority; preserve executable-price selection facts required outside economics.

- [ ] **Step 4: Verify GREEN remotely and commit**

Expected: cap, overflow, partial failure, reverse rollback, and no-mutation-before-rejection tests pass.

### Task 4: Represent Provider Truth and Block Unsupported Shapes

**Files:**
- Modify: `src/bolt_v3_providers/hyperliquid/economics.rs`
- Modify: `src/bolt_v3_providers/polymarket/economics.rs`
- Modify: `tests/hyperliquid_economics_contract.rs`
- Modify: `tests/polymarket_economics_contract.rs`
- Modify: `tests/fixtures/bolt_v3/boundary_evidence/*`
- Modify: `src/bolt_v3_providers/boundary_registry.rs`

**Interfaces:**
- Hyperliquid threshold resolution remains provider-private.
- Polymarket supports only governed exponent `1`; every other exponent is `UnsupportedExponent`.

- [ ] **Step 1: Write failing real-shape provider tests**

Add a governed nonzero-stake fixture where actual stake lies between thresholds and the active discount equals the greatest satisfied threshold. Add the contradictory-discount rejection twin. Change exponent-two coverage from success to rejection.

- [ ] **Step 2: Verify RED remotely**

Expected: mid-tier Hyperliquid response rejects under exact-row equality; Polymarket exponent two still succeeds.

- [ ] **Step 3: Implement threshold semantics and capability blocking**

Select the greatest ordered tier with `threshold <= actual`; require the venue-reported active discount to equal that tier. Reject malformed, duplicate, unordered, missing-baseline, or contradictory schedules. Delete exponent-two formula execution and synthetic success fixtures.

- [ ] **Step 4: Verify GREEN remotely and commit**

Expected: official-shaped staking fixture quotes once; contradictory state and unsupported exponent publish nothing.

### Task 5: Separate Supplemental Forecasts from Admission Evidence

**Files:**
- Modify: `src/bolt_v3_economics_runtime.rs`
- Modify: `src/economics/quote.rs`
- Test: `tests/bolt_v3_economics_runtime.rs`
- Test: `tests/economics_quote_contract.rs`

**Interfaces:**
- Required component valuation errors reject admission.
- Forecast-only valuation absence produces `forecast_complete = false` without entering or blocking the core seal.

- [ ] **Step 1: Write failing production-runtime forecast test**

Quote a valid guaranteed component plus an unvalued forecast-only component through the configured runtime. Assert the core admission succeeds and forecast is incomplete. Add the required-component rejection twin.

- [ ] **Step 2: Verify RED remotely**

Expected: the runtime valuation pre-pass rejects the forecast-only component before core degradation.

- [ ] **Step 3: Make valuation requiredness explicit before resolution**

Resolve required valuation evidence with propagated errors. Attempt supplemental valuation independently and omit only the unavailable forecast normalization while retaining its health gap. Structurally malformed components still reject.

- [ ] **Step 4: Verify GREEN remotely and commit**

Expected: missing supplemental evidence degrades only forecast; missing guaranteed evidence rejects.

### Task 6: Add One Production Composition Seam

**Files:**
- Modify: `src/bolt_v3_providers/mod.rs`
- Modify: `src/bolt_v3_providers/polymarket/economics.rs`
- Modify: `src/bolt_v3_providers/hyperliquid/economics.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Test: `tests/bolt_v3_provider_binding.rs`
- Test: `tests/wiring_registration.rs`

**Interfaces:**
- Produces: one-shot `refresh_compile_publish_once` behavior used by the production loop and deterministic fixture transport tests.
- The transport seam returns raw authoritative bytes plus receipt timestamps; it does not return domain adapters or admissions.

- [ ] **Step 1: Write failing shipped-shaped composition tracer**

Start from shipped-shaped TOML, real registry bindings, governed raw provider fixtures, and a deterministic clock. Exercise build authority → fetch → compile → publish → strategy routing → quote → admission → capital reserve → induced failure → rollback. Add malformed/stale/missing-binding twins.

- [ ] **Step 2: Verify RED remotely**

Expected: no callable one-shot production seam exists, so the tracer cannot compile or cannot populate the store.

- [ ] **Step 3: Extract the minimum production seam**

Factor one refresh iteration out of the spawned loop. Production calls it with existing transports; tests supply raw governed fixture responses. Do not add a second adapter builder, secret source, or domain mock.

- [ ] **Step 4: Verify GREEN remotely and commit**

Expected: complete tracer and all fail-closed twins pass without live credentials or live submission.

### Task 7: Close the Cutover and Obtain Exact-Head Review

**Files:**
- Modify: `scripts/verify_economics_single_path.py`
- Modify: `scripts/verify_economics_dependency_direction.py`
- Modify: associated Python self-tests
- Modify: `docs/superpowers/plans/2026-07-17-venue-agnostic-economics-implementation.md` only where its evidence matrix is factually stale

**Interfaces:**
- Produces: static rejection of reintroduced raw scalar authorities, scanner reservation economics, replay policy forks, and unsupported provider success branches.

- [ ] **Step 1: Add failing static negative controls**

Add fixtures that reintroduce a copied submit notional, basket scanner reservation, replay-owned validity, an Option fallback, and an unsupported provider formula branch. Confirm each verifier fails.

- [ ] **Step 2: Update fences minimally**

Scan registry-derived provider modules and all economics consumers without hardcoded venue lists. Reject the exact duplicate-authority patterns while allowing unrelated scanner selection arithmetic.

- [ ] **Step 3: Run local non-compile verification**

Run `cargo fmt --all -- --check`, `git diff --check`, `just source-fence-static`, and the loopback-enabled `just ci-lint-workflow`.

- [ ] **Step 4: Publish coherent exact head and run governed remote verification**

Require root gate, complete nextest archive, backtester archive/gate, coverage, actionlint, clippy, deny, source-fence, build, and host-health at the exact head.

- [ ] **Step 5: Request exact-head adversarial closure review**

Require prior-finding disposition and a binary closure verdict. Fix every valid finding before requesting the mandated native reviewer. Do not merge.

