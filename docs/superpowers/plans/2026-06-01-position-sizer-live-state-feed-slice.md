# Position Sizer Live State Feed Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the next production slice for PR #507 by feeding submit admission from live NT account, portfolio, order-lifecycle, and configured binary-product evidence without letting the feed own the reservation ledger.

**Architecture:** NT remains the authority for account, portfolio, order, position, and cache facts. Bolt owns the reservation ledger and composes final sizing state inside `BoltV3SubmitAdmissionState`, so feed updates cannot overwrite or fabricate ledger evidence. This slice opens admission only after live NT-derived components are present and an explicit startup/reconnect rebuild has reconciled open-order reservations.

**Tech Stack:** Rust, NautilusTrader Rust msgbus/cache APIs, existing `bolt_v3_submit_admission`, `bolt_v3_position_sizer`, `bolt_v3_sizing_state`, `bolt_v3_position_sizer_runtime_feed`, `cargo test --locked`, GitHub CI after a single meaningful push.

---

## Approval Status

This plan is not approved for implementation yet.

- Gemini custom-review job `d04d72ec-a1c9-43c7-b966-59842380af3d` returned `REQUEST_CHANGES`.
- This revision addresses Gemini's blockers by adding explicit subscriptions, moving reservation-snapshot ownership into submit admission, seeding runtime feed state from cache, removing unimplementable residual-liability work from this slice, and requiring configured YES/NO metadata before live wiring.
- Claude custom-review job `59712ded-3ad2-4c32-b7a3-c1f43476e400` returned `REQUEST_CHANGES`.
- This revision also addresses Claude's blockers: the old direct state API must be removed or routed through composition, NT symbols and field mappings are pinned, unsupported open orders get a concrete fail-closed rebuild input, cache/live event ordering is set-based, subscription drop behavior is tested, and empty-ledger timestamp semantics are explicit.
- Claude custom-review job `a66f16b7-84f2-46a9-bcd1-46b58ea9df22` returned `REQUEST_CHANGES`.
- This revision addresses that review by preserving `client_order_id` in startup/reconnect rebuilt reservations, labeling NT free-balance-as-allowance as a remaining production gap, and pinning the remaining test/API/timestamp details before implementation.
- Implementation must wait for Claude plan approval or explicit user waiver.

## Current Verified State

- Branch: `codex/nt-position-sizer-production-slice`.
- PR: #507, draft.
- Latest pushed code-review-fix head before this plan revision: `de9355c913f177659dd0697ab7287e9b40faefcd`.
- Committing this plan revision will create a newer PR head; do not request external implementation review until CI is green on that newer head.
- `PositionSizerRuntimeFeed` currently subscribes only to `OrderEventAny` and releases committed reservations for terminal order events.
- `BoltV3SubmitAdmissionState` owns `PositionSizingAdmissionGate`, `client_order_reservations`, and current `NtDerivedSizingState`.
- `BoltV3SubmitAdmissionState::update_position_sizing_state(NtDerivedSizingState)` currently trusts caller-supplied reservation evidence and must not remain as a public bypass after this slice.
- `rebuild_position_sizing_open_order_reservations(...)` already exists and keeps the gate unreconciled when state is missing or rebuild fails.

## Scope For This Slice

1. Add explicit runtime-feed subscriptions for NT account states, portfolio snapshots, order events, and position events.
2. Add a feed-owned component builder for portfolio/product/order-lifecycle components only.
3. Add a submit-admission API that composes `NtDerivedSizingState` internally using the feed components plus a ledger snapshot owned by submit admission.
4. Add configured prediction-market binary product metadata to the capital-pool config so the live feed has YES/NO instrument ids and collateral group id without hardcodes.
5. Seed the feed from NT cache at startup/reconnect before rebuild, including open-order count and configured product identity.
6. Keep the gate closed until both component state and open-order reservation rebuild succeed.
7. Track live order count from account-bound NT order events and keep account-less non-denied events fail-closed.
8. Keep unsupported residual fill revalue, maker quote sets, replace/amend, non-binary calculators, and venue allowance gaps explicitly closed.
9. Do not leave any public API that can inject arbitrary reservation-ledger evidence into submit admission.

## Explicit Non-Scope After Gemini Review

These remain production gaps after this slice:

- Residual liability revalue from partial fills. It needs either cached order details from NT or submit-time liability metadata; this plan does not fake it from fill deltas.
- Dynamic market rotation metadata. This slice requires configured binary product metadata in TOML; later work can replace that with NT market-selection state.
- Conditional-token allowance truth. Until NT or the adapter exposes allowance evidence, sells must remain fail-closed when allowance cannot be proven.
- PUSD allowance/spendability truth. This slice may use NT-reported `AccountState` free collateral as the best available buy-side collateral evidence, but it does not prove separate on-chain allowance or venue spendability. Final production readiness still needs adapter/venue allowance evidence or a fail-closed zero allowance until proven.
- Maker quote-set simultaneous adverse-fill reservations.
- Safe `ReplaceSubmit`.
- Cancel/flatten halt actions.
- Spot leverage, futures/perps, and options calculators.
- Final production readiness or merge readiness.

## Files

- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_validate.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `src/bolt_v3_position_sizer_runtime_feed.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_sizing_state.rs`
- Modify: `src/nt_runtime_capture.rs`
- Modify: `config/root.example.toml`
- Modify: `tests/fixtures/bolt_v3/root.toml`
- Modify: `tests/config_parsing.rs`
- Modify: `tests/bolt_v3_position_sizer_runtime_feed.rs`
- Modify: `tests/bolt_v3_submit_admission.rs`
- Modify: `specs/506-nt-position-sizer-submit/spec.md`
- Modify: `specs/506-nt-position-sizer-submit/tasks.md`

## Task 1: Configured Binary Product Metadata

**Files:**
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_validate.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `config/root.example.toml`
- Modify: `tests/fixtures/bolt_v3/root.toml`
- Test: `tests/config_parsing.rs`

- [ ] **Step 1: RED parse configured binary product metadata**

Add a test proving each submit-enforced `prediction_market_binary` capital pool requires product metadata:

```rust
#[test]
fn capital_pool_prediction_market_binary_metadata_parses() {
    let root = parse_fixture_root();
    let pool = root.risk.capital_pools.as_ref().unwrap()
        .iter()
        .find(|pool| pool.pool_id == "polymarket-prediction-live")
        .unwrap();
    let product = pool.prediction_market_binary.as_ref().unwrap();
    assert_eq!(product.yes_instrument_id.to_string(), "condition-fixture-yes.POLYMARKET");
    assert_eq!(product.no_instrument_id.to_string(), "condition-fixture-no.POLYMARKET");
    assert_eq!(product.collateral_coupled_group_id, "condition-fixture");
}
```

Run:

```bash
cargo test --locked --test config_parsing capital_pool_prediction_market_binary_metadata_parses -- --nocapture
```

Expected before implementation: FAIL because `CapitalPoolBlock` has no `prediction_market_binary` field.

- [ ] **Step 2: GREEN add config shape and validation**

Add:

```rust
pub struct PredictionMarketBinaryProductBlock {
    pub yes_instrument_id: InstrumentId,
    pub no_instrument_id: InstrumentId,
    pub collateral_coupled_group_id: String,
}
```

Add `prediction_market_binary: Option<PredictionMarketBinaryProductBlock>` to `CapitalPoolBlock`.

Validation rules:
- required when `product_kind == "prediction_market_binary"` and `enforce_submit_admission == true`;
- rejected when `product_kind != "prediction_market_binary"`;
- YES and NO instrument ids must differ;
- `collateral_coupled_group_id` must be non-empty.

Add TOML:

```toml
[risk.capital_pools.prediction_market_binary]
yes_instrument_id = "condition-fixture-yes.POLYMARKET"
no_instrument_id = "condition-fixture-no.POLYMARKET"
collateral_coupled_group_id = "condition-fixture"
```

- [ ] **Step 3: RED live-node config carries product snapshot**

Add a test for `position_sizer_runtime_feed_config_from_loaded(...)` asserting it carries venue id, account id, collateral currency, and a `ProductSizingSnapshot::PredictionMarketBinary` whose YES/NO ids come from TOML.

The test must pass an explicit `config_loaded_at_ns` or `startup_observed_at_ns`; the product snapshot timestamp must come from that value, not from `0`.

Run:

```bash
cargo test --locked --lib position_sizer_runtime_feed_config_carries_configured_binary_product -- --nocapture
```

Expected before implementation: FAIL because the feed config carries only `account_id`.

- [ ] **Step 4: GREEN extend runtime feed config**

Change `PositionSizerRuntimeFeedConfig` to:

```rust
pub struct PositionSizerRuntimeFeedConfig {
    pub venue_id: String,
    pub account_id: AccountId,
    pub collateral_currency: String,
    pub product_state: ProductSizingSnapshot,
    pub startup_observed_at_ns: u64,
}
```

Build `product_state` from TOML with `source = "bolt_configured_binary_product"`, `observed_at_ns = startup_observed_at_ns`, `yes_position = 0`, `no_position = 0`, `pusd_allowance = 0`, and `conditional_token_allowance = 0`. Later NT account/position events raise only the fields they prove.

Update every construction site in the same slice:
- `position_sizer_runtime_feed_config_from_loaded(...)` in `src/bolt_v3_live_node.rs`;
- constructors and direct config builders in `tests/bolt_v3_position_sizer_runtime_feed.rs`;
- config parsing fixtures in `tests/fixtures/bolt_v3/root.toml` and `config/root.example.toml`;
- any direct `PositionSizerRuntimeFeedConfig { ... }` builders found by `rg -n "PositionSizerRuntimeFeedConfig \\{"`.

## Task 2: Submit Admission Owns Final State Composition

**Files:**
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_position_sizer.rs`
- Modify: `src/bolt_v3_sizing_state.rs`
- Test: `tests/bolt_v3_submit_admission.rs`

- [ ] **Step 1: RED direct state update cannot inject ledger evidence**

Add `direct_state_update_discards_hostile_reservation_evidence`.

```rust
#[test]
fn direct_state_update_discards_hostile_reservation_evidence() {
    let admission = Arc::new(position_sized_admission());
    let mut hostile_state = fresh_sizing_state(1_000);
    hostile_state.reservation_snapshot.source = "hostile_feed".to_string();
    hostile_state.reservation_snapshot.observed_at_ns = 9_999;
    hostile_state.reservation_snapshot.all_live_reservations_attributed = true;

    admission.update_position_sizing_state(hostile_state);

    let state = admission.position_sizer_state_snapshot().unwrap();
    assert_eq!(state.reservation_snapshot.source, "bolt_reservation_ledger");
    assert_eq!(state.reservation_snapshot.observed_at_ns, 1_000);
    assert_eq!(state.reservation_snapshot.all_live_reservations_attributed, false);
    assert_eq!(admission.position_sizer_reconciled(), Some(false));
}
```

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission direct_state_update_discards_hostile_reservation_evidence -- --nocapture
```

Expected before implementation: FAIL because `update_position_sizing_state(NtDerivedSizingState)` currently trusts the caller-supplied reservation snapshot and can mark the stored state attributed.

- [ ] **Step 2: RED component update cannot clobber ledger evidence**

Add:

```rust
#[test]
fn nt_component_update_preserves_submit_owned_reservation_snapshot() {
    let admission = Arc::new(position_sized_admission());
    admission.update_position_sizing_nt_components(fresh_components(1_000));
    let state = admission.position_sizer_state_snapshot().unwrap();
    assert_eq!(state.reservation_snapshot.source, "bolt_reservation_ledger");
    assert_eq!(state.reservation_snapshot.all_live_reservations_attributed, false);
    assert_eq!(admission.position_sizer_reconciled(), Some(false));
}
```

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission nt_component_update_preserves_submit_owned_reservation_snapshot -- --nocapture
```

Expected before implementation: FAIL because component update APIs and state accessors do not exist.

- [ ] **Step 3: GREEN add component API and sanitize legacy state update**

Add:

```rust
pub struct BoltV3SubmitPositionSizingNtComponents {
    pub source: String,
    pub observed_at_ns: u64,
    pub portfolio: PortfolioSizingSnapshot,
    pub order_lifecycle: OrderLifecycleSizingSnapshot,
    pub product_state: ProductSizingSnapshot,
    pub loss_snapshot: Option<LossSnapshot>,
}
```

Add:

```rust
pub fn update_position_sizing_nt_components(&self, components: BoltV3SubmitPositionSizingNtComponents);
pub fn position_sizer_state_snapshot(&self) -> Option<NtDerivedSizingState>;
pub fn position_sizer_state_observed_at_ns(&self) -> Option<u64>;
```

Rewrite `update_position_sizing_state(NtDerivedSizingState)` as a compatibility wrapper that converts the incoming value into `BoltV3SubmitPositionSizingNtComponents` and discards the incoming `reservation_snapshot`. Do not copy `state.reservation_snapshot` anywhere. New runtime code must call `update_position_sizing_nt_components(...)`; keep the old method only until existing tests are migrated, and it must be safe if any caller still reaches it.

Submit admission must keep the existing low-churn internal representation `state: Option<NtDerivedSizingState>`, add `latest_reservation_mutation_observed_at_ns: Option<u64>`, and route every write through one private helper, for example:

```rust
fn compose_position_sizing_state_from_components(
    components: BoltV3SubmitPositionSizingNtComponents,
    gate_reconciled: bool,
    latest_reservation_mutation_observed_at_ns: Option<u64>,
) -> NtDerivedSizingState;
```

Do not mutate `position_sizer.state` directly outside this helper except to clear it. Submit admission must compose `NtDerivedSizingState` internally and set:
- reservation source: `bolt_reservation_ledger`;
- reservation timestamp:
  - if a ledger mutation exists: `max(components.observed_at_ns, latest_reservation_mutation_observed_at_ns)`;
  - if no ledger mutation exists yet: `components.observed_at_ns`;
- `all_live_reservations_attributed`: `position_sizer.gate.is_reconciled()`.

If the gate is unreconciled, the component state may be stored for rebuild, but submit admission must still reject new orders through the existing reconciliation gate.

After this step, run:

```bash
rg -n "state\\.reservation_snapshot|reservation_snapshot: state|reservation_snapshot: .*incoming" src/bolt_v3_submit_admission.rs
```

Expected after implementation: no matches showing direct caller-supplied reservation evidence copied into stored sizing state.

- [ ] **Step 4: RED rebuild flips reservation snapshot attribution**

Add a test proving `rebuild_position_sizing_open_order_reservations(Vec::new(), now)` refreshes the stored state so `reservation_snapshot.all_live_reservations_attributed == true` after accepted rebuild.

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission rebuild_refreshes_submit_owned_reservation_snapshot -- --nocapture
```

Expected before implementation: FAIL because rebuild does not refresh the stored state snapshot.

- [ ] **Step 5: GREEN refresh state after rebuild and lifecycle mutations**

After successful rebuild, terminal release, rollback, or accepted revalue paths that already exist in this module, update only the submit-owned reservation snapshot fields on the stored state. Do not let feed-provided state supply reservation evidence. Track `latest_reservation_mutation_observed_at_ns: Option<u64>` inside the submit-admission position-sizer state; set it only from accepted rebuild/lifecycle mutations, never from feed components.

- [ ] **Step 6: GREEN migrate tests away from arbitrary state injection**

Replace existing test setup calls to `admission.update_position_sizing_state(fresh_sizing_state(...))` with a helper that builds `BoltV3SubmitPositionSizingNtComponents` and calls `update_position_sizing_nt_components(...)`. The only remaining direct-state test should be `direct_state_update_discards_hostile_reservation_evidence`.

## Task 3: Runtime Feed Subscribes And Publishes Components

**Files:**
- Modify: `src/nt_runtime_capture.rs`
- Modify: `src/bolt_v3_position_sizer_runtime_feed.rs`
- Test: `tests/bolt_v3_position_sizer_runtime_feed.rs`

- [ ] **Step 1: RED compile against the exact NT subscription surface**

Add `runtime_feed_uses_verified_nt_msgbus_symbols`.

The implementation must compile with these exact NT symbols:

```rust
use nautilus_common::msgbus::{
    TypedHandler,
    subscribe_account_state,
    subscribe_order_events,
    subscribe_portfolio_snapshot,
    subscribe_position_events,
    unsubscribe_account_state,
    unsubscribe_order_events,
    unsubscribe_portfolio_snapshot,
    unsubscribe_position_events,
};
use nautilus_model::events::{
    AccountState,
    OrderEventAny,
    PortfolioSnapshot,
    PositionEvent,
};
```

Use these Bolt pattern helpers:

```rust
use crate::nt_runtime_capture::{
    account_states_pattern,
    order_events_pattern,
    portfolio_snapshots_pattern,
    position_events_pattern,
};
```

Change `src/nt_runtime_capture.rs` so `account_states_pattern()` is `pub(crate)`, matching the already crate-visible `order_events_pattern()`, `position_events_pattern()`, and `portfolio_snapshots_pattern()`.

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed runtime_feed_uses_verified_nt_msgbus_symbols -- --nocapture
```

Expected before implementation: FAIL to compile or fail behaviorally because the runtime feed imports and subscribes only `OrderEventAny`.

- [ ] **Step 2: RED subscription handles account and portfolio topics**

Add `subscribed_account_and_portfolio_events_publish_sizing_components`.

Behavior:
- Build a feed with configured venue/account/currency/product metadata.
- Subscribe the feed.
- Publish matching NT `AccountState` on `events.account.*` and matching `PortfolioSnapshot` on `events.portfolio.*`.
- Assert `admission.position_sizer_state_observed_at_ns() == Some(newest_ts)`.
- Assert `admission.position_sizer_reconciled() == Some(false)`.

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed subscribed_account_and_portfolio_events_publish_sizing_components -- --nocapture
```

Expected before implementation: FAIL because the feed only subscribes to order events.

- [ ] **Step 3: GREEN wire explicit subscriptions**

Make `account_states_pattern()` public within the crate in `src/nt_runtime_capture.rs`:

```rust
pub(crate) fn account_states_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern(ACCOUNT_STATES_PATTERN)
}
```

`PositionSizerRuntimeFeedSubscription` must hold:

```rust
order_events: Option<TypedHandler<OrderEventAny>>,
position_events: Option<TypedHandler<PositionEvent>>,
account_states: Option<TypedHandler<AccountState>>,
portfolio_snapshots: Option<TypedHandler<PortfolioSnapshot>>,
```

Subscribe/unsubscribe all four handlers using NT msgbus functions:
- `subscribe_order_events`
- `subscribe_position_events`
- `subscribe_account_state`
- `subscribe_portfolio_snapshot`

`Drop` for `PositionSizerRuntimeFeedSubscription` must unsubscribe all non-`None` handlers with:

```rust
unsubscribe_order_events(order_events_pattern(), &order_events);
unsubscribe_position_events(position_events_pattern(), &position_events);
unsubscribe_account_state(account_states_pattern(), &account_states);
unsubscribe_portfolio_snapshot(portfolio_snapshots_pattern(), &portfolio_snapshots);
```

- [ ] **Step 4: RED subscription drop unsubscribes every handler**

Add `position_sizer_runtime_subscription_drop_unsubscribes_all_handlers`.

Behavior:
- Subscribe feed.
- Drop the returned `PositionSizerRuntimeFeedSubscription`.
- Publish account state, portfolio snapshot, position event, and order event that would otherwise mutate the feed.
- Assert submit admission state and live reservation state do not change after drop.

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed position_sizer_runtime_subscription_drop_unsubscribes_all_handlers -- --nocapture
```

Expected before implementation: FAIL because only order-event subscription drop exists today.

- [ ] **Step 5: RED partial account/portfolio evidence does not publish**

Add `feed_waits_for_matching_account_and_portfolio_before_publish`.

Behavior:
- Matching account alone does not update submit admission state.
- Matching portfolio alone does not update submit admission state.
- Mismatched account id does not update state.
- Matching account with no configured collateral-currency balance does not update state.
- Matching portfolio with no configured collateral-currency total equity does not update state.

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed feed_waits_for_matching_account_and_portfolio_before_publish -- --nocapture
```

- [ ] **Step 6: GREEN field-by-field account/portfolio mapping**

Add a feed-owned builder with only non-ledger components:

```rust
struct PositionSizerRuntimeComponentBuilder {
    latest_account_state: Option<AccountState>,
    latest_portfolio_snapshot: Option<PortfolioSnapshot>,
    latest_portfolio: Option<PortfolioSizingSnapshot>,
    order_lifecycle: OrderLifecycleSizingSnapshot,
    product_state: ProductSizingSnapshot,
}
```

The builder constructor must take `startup_observed_at_ns` from `PositionSizerRuntimeFeedConfig` and seed `order_lifecycle.observed_at_ns = startup_observed_at_ns`; empty lifecycle evidence must not use timestamp `0`.

Map NT account and portfolio events exactly:

- accept `AccountState` only when `account_state.account_id == config.account_id`;
- find the collateral balance with `balance.currency.code.as_str() == config.collateral_currency`;
- set `PortfolioSizingSnapshot.free_collateral = balance.free.as_decimal()`;
- accept `PortfolioSnapshot` only when `portfolio_snapshot.account_id == config.account_id`;
- find total equity with `money.currency.code.as_str() == config.collateral_currency`;
- set `PortfolioSizingSnapshot.total_equity = money.as_decimal()`;
- set `PortfolioSizingSnapshot.account_id = config.account_id.to_string()`;
- set `PortfolioSizingSnapshot.venue_id = config.venue_id.clone()`;
- set `PortfolioSizingSnapshot.collateral_currency = config.collateral_currency.clone()`;
- set component `observed_at_ns = max(account_state.ts_event.as_u64(), portfolio_snapshot.ts_event.as_u64(), order_lifecycle.observed_at_ns, product_state.observed_at_ns)`;
- if account id, collateral balance, portfolio account id, or total equity currency is missing or mismatched, do not publish a component state; admission remains closed or stale.

When both matching account and portfolio evidence are present, call `update_position_sizing_nt_components(...)`. Set `pusd_allowance = matching_account_balance.free.as_decimal()` only as this slice's explicit approximation from NT-reported free collateral. This is not proof of separate on-chain allowance or venue spendability; final production readiness still needs adapter/venue allowance evidence or a fail-closed zero allowance until proven. Conditional token allowance remains zero unless later proven by NT.

- [ ] **Step 7: RED account currency mismatch remains closed**

Add `feed_ignores_account_state_for_other_collateral_currency`.

Behavior:
- Publish matching portfolio total equity in configured collateral currency.
- Publish matching account state where the only balance is a different currency.
- Assert `position_sizer_state_snapshot()` remains `None` or keeps its prior stale state and new submit admission stays rejected.

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed feed_ignores_account_state_for_other_collateral_currency -- --nocapture
```

Expected before implementation: FAIL because the feed has no account/currency mapper.

## Task 4: Startup/Reconnect Cache Seed Boundary

**Files:**
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_position_sizer_runtime_feed.rs`
- Test: `tests/bolt_v3_submit_admission.rs`

Task boundary:
- direct submit-admission rebuild semantics are tested in `tests/bolt_v3_submit_admission.rs`;
- the live-node NT cache reader is added only after the direct snapshot API is pinned, then tested through the live-node entrypoint.

- [ ] **Step 1: RED direct rebuild stays closed without component state**

Add `position_sizer_direct_rebuild_keeps_gate_closed_without_components`.

Behavior:
- Build submit admission with submit sizing enabled.
- Call `rebuild_position_sizing_open_order_reservations(Vec::new(), now)` before account/portfolio/product components are published.
- Assert rebuild rejected with `MissingEvidence`.
- Assert `position_sizer_reconciled() == Some(false)`.

- [ ] **Step 2: GREEN explicit rebuild entrypoint**

Add:

```rust
pub fn rebuild_position_sizer_from_nt_cache(&self, now_ns: u64) -> BoltV3SubmitPositionSizingRebuildDecision;
```

The method reads NT cache only through `self.node.kernel().cache()` and calls `rebuild_position_sizing_open_order_snapshot(...)` after the direct API is added below. It must not open admission if component state is absent.

- [ ] **Step 3: RED unattributed cache open order fails closed**

Add `unattributed_cache_open_order_keeps_position_sizer_unreconciled`.

Behavior:
- Seed matching account/portfolio/product components.
- Simulate NT cache reporting one live open order whose client order id is not present in Bolt submit admission's `client_order_reservations`.
- Call the startup/reconnect rebuild entrypoint.
- Assert rebuild rejected with `ReservationRejectionReason::MissingEvidence`.
- Assert `position_sizer_reconciled() == Some(false)`.
- Assert the stored `reservation_snapshot.all_live_reservations_attributed == false`.

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission unattributed_cache_open_order_keeps_position_sizer_unreconciled -- --nocapture
```

Expected before implementation: FAIL because the current rebuild API only accepts attributed reservation requests and has no explicit unsupported-open-order marker.

Add `rebuild_snapshot_preserves_client_order_id_for_terminal_release`.

Behavior:
- Seed matching component state.
- Call `rebuild_position_sizing_open_order_snapshot(...)` with one attributed `BoltV3SubmitPositionSizingOpenOrderReservation` whose `client_order_id == "client-1"` and `submit_reservation_id == "reservation-1"`.
- Assert rebuild accepted and `position_sizer_live_reserved_liability()` is non-zero.
- Apply a terminal lifecycle/order event for `client_order_id == "client-1"`.
- Assert `position_sizer_live_reserved_liability() == Some(Decimal::ZERO)`.

Expected before implementation: FAIL because the snapshot API does not exist yet; it must prove rebuild preserves the client-order index used by terminal release.

- [ ] **Step 4: GREEN add explicit open-order snapshot API**

Add this submit-admission input type. Use the existing `BoltV3SubmitPositionSizingOpenOrderReservation` type inside the snapshot so rebuilt reservations preserve `client_order_id` and `submit_reservation_id`.

```rust
pub struct BoltV3SubmitPositionSizingOpenOrderSnapshot {
    pub observed_at_ns: u64,
    pub evidence_label: String,
    pub all_open_orders_attributed: bool,
    pub reservations: Vec<BoltV3SubmitPositionSizingOpenOrderReservation>,
}
```

Add:

```rust
pub fn rebuild_position_sizing_open_order_snapshot(
    &self,
    snapshot: BoltV3SubmitPositionSizingOpenOrderSnapshot,
    now_ns: u64,
) -> BoltV3SubmitPositionSizingRebuildDecision;
```

Rules:
- if `snapshot.all_open_orders_attributed == false`, set the gate unreconciled, clear `client_order_reservations`, refresh stored state with `reservation_snapshot.source = snapshot.evidence_label`, `reservation_snapshot.observed_at_ns = snapshot.observed_at_ns`, `reservation_snapshot.all_live_reservations_attributed = false`, and return rejected with `ReservationRejectionReason::MissingEvidence`;
- if component state is absent, also return rejected with `MissingEvidence`;
- otherwise rebuild from `snapshot.reservations`, converting each item into the internal reservation request while preserving `client_order_id` in `client_order_reservations`;
- keep existing `rebuild_position_sizing_open_order_reservations(Vec<BoltV3SubmitPositionSizingOpenOrderReservation>, now_ns)` as a delegating helper with `all_open_orders_attributed = true` and `evidence_label = "bolt_recovered_open_order_reservations"`.

- [ ] **Step 5: RED cache seed populates open-order lifecycle before rebuild**

Add `position_sizer_cache_seed_updates_open_order_lifecycle_and_rebuilds_empty`.

Behavior:
- Seed feed components with matching account/portfolio.
- Seed cache with zero open orders.
- Call the rebuild entrypoint.
- Assert accepted rebuild, reconciled gate, and state evidence showing open_order_count `0` with source `nt_open_order_cache`.

- [ ] **Step 6: GREEN seed feed from cache**

Before calling submit-admission rebuild, seed the feed builder from NT cache:
- current open-order count for the configured account/venue;
- position inventory for configured YES/NO ids when NT cache has positions;
- otherwise YES/NO positions remain zero;
- if any open order cannot be attributed to a known client order id and configured product metadata, call rebuild with an unsupported reservation marker that fails closed.

- [ ] **Step 7: RED cache seed and live event order is idempotent**

Add `cache_seed_and_concurrent_order_event_do_not_double_count`.

Behavior:
- Publish a live `OrderAccepted` event for client order id `A`.
- Run cache seed where NT cache also reports open order `A`.
- Assert open-order count remains `1`, not `2`.
- Publish a terminal event for `A`.
- Run cache seed again with stale cache still reporting `A`.
- Assert terminal evidence wins and open-order count remains `0`.

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed cache_seed_and_concurrent_order_event_do_not_double_count -- --nocapture
```

Expected before implementation: FAIL because cache seed and live event ordering is not represented.

- [ ] **Step 8: GREEN deterministic cache/live ordering**

Use set semantics:

```rust
struct PositionSizerRuntimeComponentBuilder {
    live_order_ids: BTreeSet<String>,
    terminal_order_ids_seen: BTreeSet<String>,
    // existing account, portfolio, lifecycle, product fields
}
```

When seeding cache:

```rust
let merged_live_order_ids = cache_open_ids
    .union(&self.live_order_ids)
    .cloned()
    .collect::<BTreeSet<String>>();
self.live_order_ids = merged_live_order_ids
    .difference(&self.terminal_order_ids_seen)
    .cloned()
    .collect::<BTreeSet<String>>();
```

Implement the above with normal `BTreeSet` operations; do not rely on event arrival order. `order_lifecycle.open_order_count` is `live_order_ids.len()`. A terminal event inserts into `terminal_order_ids_seen` and removes from `live_order_ids`; a later stale cache seed cannot resurrect that id.

## Task 5: Order Lifecycle Count And Fail-Closed Unsupported Events

**Files:**
- Modify: `src/bolt_v3_position_sizer_runtime_feed.rs`
- Test: `tests/bolt_v3_position_sizer_runtime_feed.rs`

- [ ] **Step 1: RED account-bound live order events update open count**

Add `account_bound_live_order_events_update_open_order_count`.

Behavior:
- After component state is publishable, send `OrderSubmitted` or `OrderAccepted` with configured account.
- Assert next state has `order_lifecycle.open_order_count == 1`.
- Send terminal event for the same client order id.
- Assert open count returns to `0` and existing terminal reservation release behavior still works.

- [ ] **Step 2: GREEN order lifecycle map**

Track account-bound live client order ids in the feed. Terminal events remove matching entries after existing reservation-release logic runs. Account-less non-denied events must not mutate the map or published components.

- [ ] **Step 3: REGRESSION partial fill is explicitly ignored without liability metadata**

Add `partial_fill_without_liability_metadata_does_not_revalue_reservation`.

Behavior:
- Commit a reservation.
- Send `OrderFilled` for the configured account.
- Assert reserved liability is unchanged and no revalue occurs.

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed partial_fill_without_liability_metadata_does_not_revalue_reservation -- --nocapture
```

Expected before implementation: this may already pass because current code ignores partial fills. Keep it as a regression lock, not a RED gate.

- [ ] **Step 4: GREEN keep residual revalue out of scope**

Do not infer residual liability from fill deltas. Add a code comment at the early return that says residual revalue requires cached order details or submit-time liability metadata and is intentionally fail-closed in this slice.

## Task 6: Spec/Tasks And Focused Verification

**Files:**
- Modify: `specs/506-nt-position-sizer-submit/spec.md`
- Modify: `specs/506-nt-position-sizer-submit/tasks.md`

- [ ] **Step 1: Update docs without production overclaim**

Docs must say this slice adds live NT component feed plus startup/reconnect rebuild, but does not complete production-grade sizing.

- [ ] **Step 2: Focused local verification**

Run only focused tests before the single push:

```bash
cargo test --locked --test config_parsing capital_pool_prediction_market_binary_metadata_parses -- --nocapture
cargo test --locked --test bolt_v3_position_sizer_runtime_feed -- --nocapture
cargo test --locked --test bolt_v3_submit_admission -- --nocapture
cargo test --locked --lib position_sizer_runtime_feed_config_carries_configured_binary_product -- --nocapture
cargo test --locked --lib -- --nocapture
cargo clippy --locked --lib -- -D warnings
cargo fmt --check
just source-fence
```

- [ ] **Step 3: Push once and use CI as source of truth**

After focused tests pass, push one commit and verify PR #507 CI at the pushed head. Do not request external implementation review while CI is pending or red.

- [ ] **Step 4: External review**

After CI is green on the pushed head, request at least Claude and one trusted fallback review for the implemented slice. Include this plan, the exact PR head SHA, and the changed files in the review request. Do not merge without explicit user approval.

## Production Gap Remaining After This Plan

Even if every task passes, the system is not production-grade until these are implemented and reviewed:

- residual liability revalue from partial fills using authoritative order state;
- existing position and existing open-order liability import for non-empty live accounts;
- safe replace/amend reservation transitions;
- maker quote-set reservation of simultaneous adverse fills;
- conditional-token allowance evidence;
- PUSD allowance/spendability evidence separate from NT account free balance;
- dynamic market metadata from NT/market-selection state;
- cancel/flatten/halt operations tied to loss governor and sizer failures;
- calculators for leveraged spot, futures/perps, and options;
- live reconnect tests against the actual NT runtime path.
