# Position Sizer Fill Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Revalue prediction-market live order reservations from authoritative NT fill events using Bolt-owned submit-time liability metadata.

**Architecture:** Submit admission remains the owner of reservation-ledger state and fill attribution. Runtime feed only filters NT order events by account and forwards fill facts; admission checks the fill against the original admitted order metadata, de-duplicates trade IDs, computes residual worst-case liability, and applies the existing reservation-ledger lifecycle path.

**Liability Semantics:** The current prediction-market calculator treats max fee and max slippage as per-order additive ceilings. Partial-fill revalue removes only the filled portion of base per-unit liability and keeps the per-order additive ceiling until terminal/full fill. This is conservative for this taker slice; maker quote sets and replace/amend may need different additive semantics later.

**Tech Stack:** Rust, NautilusTrader Rust order events, `rust_decimal`, existing Bolt v3 submit admission, existing capital reservation ledger, cargo tests.

---

## Approval Gate

Do not implement this plan until Claude adversarial review approves it for this exact branch head. This plan is a production-sizer slice, not the full production-grade positional sizer. It closes residual liability revalue for live prediction-market taker orders. It does not close restart attribution, replace/amend, maker quote-set reservation, dynamic market metadata, allowance proof, halt/flatten actions, or non-binary calculators.

## Files

- Modify: `src/bolt_v3_submit_admission.rs`
  - Store submit-time fill metadata alongside each live client-order reservation.
  - Add a public fill-update API that admission owns.
  - Keep old lifecycle APIs intact for terminal events and tests.
- Modify: `src/bolt_v3_position_sizer_runtime_feed.rs`
  - Convert matching NT `OrderFilled` events into admission fill updates.
  - Keep account/instrument mismatches non-mutating.
  - Update order lifecycle open-count only when admission accepts a full-fill release.
- Modify: `tests/bolt_v3_submit_admission.rs`
  - Add direct admission tests for partial fill revalue, duplicate fill idempotency, full fill release, and mismatch rejection.
- Modify: `tests/bolt_v3_position_sizer_runtime_feed.rs`
  - Replace the old "partial fill is non-mutating" regression with runtime feed tests proving partial fills revalue and full fills release.
- Modify: `specs/506-nt-position-sizer-submit/spec.md`
  - Move residual partial-fill revalue from remaining production gap to implemented in this slice.
- Modify: `specs/506-nt-position-sizer-submit/tasks.md`
  - Mark residual partial-fill revalue complete and keep the remaining production gaps explicit.
- Modify: `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`
  - Classify new runtime evidence labels.

---

## Task 1: Admission Fill Metadata And Partial Revalue

**Files:**
- Modify: `tests/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_submit_admission.rs`

- [ ] **Step 1: RED direct partial fill revalues live reservation**

Add this test near `configured_submit_sizer_keeps_committed_reservation_until_terminal_lifecycle_release`. Also import `PositionSizingLifecycleAction` from `bolt_v3_position_sizer` in this test module.

```rust
#[test]
fn configured_submit_sizer_revalues_residual_liability_from_fill_metadata() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(4, 0),
            observed_at_ns: 1_100,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Revalued);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}
```

Add a second test proving the fee/slippage add-on remains a per-order ceiling after partial fills:

```rust
#[test]
fn configured_submit_sizer_keeps_per_order_additive_liability_after_partial_fill() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(9, 0),
            observed_at_ns: 1_100,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Revalued);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(7, 1))
    );
}
```

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission configured_submit_sizer_ -- --nocapture
```

Expected before implementation: FAIL because `BoltV3SubmitPositionSizingFillUpdate` and `apply_position_sizing_fill_update` do not exist, and fill metadata is not stored with the reservation index.

- [ ] **Step 2: GREEN add fill-update type and metadata fields**

In `src/bolt_v3_submit_admission.rs`, import `BTreeSet` next to `BTreeMap`, extend the reservation index, and add the fill update type:

```rust
use std::collections::{BTreeMap, BTreeSet};
```

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitReservationIndex {
    submit_reservation_id: String,
    collateral_group_id: String,
    fill_metadata: Option<BoltV3SubmitReservationFillMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitReservationFillMetadata {
    instrument_id: String,
    side: BoltV3CompiledOrderSide,
    original_quantity: Decimal,
    filled_quantity: Decimal,
    liability_factor: Decimal,
    additive_liability: Decimal,
    last_lifecycle_observed_at_ns: u64,
    seen_trade_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingFillUpdate {
    pub client_order_id: String,
    pub trade_id: String,
    pub instrument_id: String,
    pub side: BoltV3CompiledOrderSide,
    pub fill_quantity: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
}
```

Also extend `BoltV3SubmitPositionSizingLifecycleDecision` in this task, before any fill-update return sites are added:

```rust
pub struct BoltV3SubmitPositionSizingLifecycleDecision {
    pub accepted: bool,
    pub unknown_reservation: bool,
    pub action: PositionSizingLifecycleAction,
}
```

`unknown()` must return `action: PositionSizingLifecycleAction::None`. Every existing construction site in `apply_position_sizing_lifecycle_update`, `apply_position_sizing_terminal_order_event`, and any new duplicate/no-op fill return must set `action` explicitly.

When inserting an admitted submit reservation, populate `fill_metadata` from the admitted request:

```rust
let admitted_quantity = decision
    .sized_quantity
    .expect("accepted position sizing decision should carry sized quantity");
let fee_slippage = position_sizer
    .policy
    .fee_slippage_policy
    .as_ref()
    .map(|policy| policy.max_fee_liability + policy.max_slippage_liability)
    .unwrap_or(Decimal::ZERO);
let liability_factor = match evidence.side.to_position_sizer() {
    IntentSide::Buy => evidence.effective_price,
    IntentSide::Sell => Decimal::ONE - evidence.effective_price,
};
```

Then store:

```rust
fill_metadata: Some(BoltV3SubmitReservationFillMetadata {
    instrument_id: request.instrument_id.clone(),
    side: evidence.side,
    original_quantity: admitted_quantity,
    filled_quantity: Decimal::ZERO,
    liability_factor,
    additive_liability: fee_slippage,
    last_lifecycle_observed_at_ns: now_ns,
    seen_trade_ids: BTreeSet::new(),
}),
```

For rebuilt open-order reservations, set `fill_metadata: None`; this keeps restart-attributed fill revalue closed until durable submit metadata exists.

- [ ] **Step 3: GREEN implement fill update through existing lifecycle path**

Add `apply_position_sizing_fill_update` on `BoltV3SubmitAdmissionState`. It must:

- return `unknown()` without mutation when the client order is unknown;
- return `unknown()` without mutation when fill metadata is absent;
- validate blank trade ID, non-positive fill quantity, instrument mismatch, and side mismatch before checking `seen_trade_ids`;
- treat duplicate trade IDs as accepted idempotent no-ops only after the update's instrument and side match the original admitted reservation metadata;
- compute `new_filled_quantity = min(original_quantity, filled_quantity + fill_quantity)`;
- compute a monotonic ledger timestamp before calling the lower-level lifecycle API:

```rust
let Some(next_observed_at_ns) = metadata
    .last_lifecycle_observed_at_ns
    .checked_add(1)
else {
    return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
};
let lifecycle_observed_at_ns = update.observed_at_ns.max(next_observed_at_ns);
let lifecycle_now_ns = now_ns.max(lifecycle_observed_at_ns);
```

- if remaining quantity is positive, call `apply_lifecycle_update` with `PositionSizingLifecycleKind::LiveResidual` and `remaining_liability = remaining_quantity * liability_factor + additive_liability`; the returned decision action must be `PositionSizingLifecycleAction::Revalued`;
- if remaining quantity is zero, call `apply_lifecycle_update` with `PositionSizingLifecycleKind::Terminal` and zero liability; the returned decision action must be `PositionSizingLifecycleAction::Released`;
- pass `lifecycle_observed_at_ns` as the lifecycle update timestamp and `lifecycle_now_ns` as the evaluation timestamp;
- update `filled_quantity`, `last_lifecycle_observed_at_ns`, and `seen_trade_ids` only after the lifecycle decision is accepted;
- remove the client-order index after an accepted full-fill release.

Use the existing `PositionSizingAdmissionGate::apply_lifecycle_update` instead of mutating the reservation ledger directly.

- [ ] **Step 4: Verify direct partial revalue passes**

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission configured_submit_sizer_ -- --nocapture
```

Expected after implementation: PASS.

---

## Task 2: Admission Idempotency, Full Fill Release, And Mismatch Safety

**Files:**
- Modify: `tests/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_submit_admission.rs`

- [ ] **Step 1: RED same-timestamp fills are applied in order**

Add:

```rust
#[test]
fn configured_submit_sizer_applies_same_timestamp_fills_in_order() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let first = BoltV3SubmitPositionSizingFillUpdate {
        client_order_id: "client-order-1".to_string(),
        trade_id: "trade-1".to_string(),
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        side: BoltV3CompiledOrderSide::Buy,
        fill_quantity: Decimal::new(4, 0),
        observed_at_ns: 1_100,
        evidence_label: "nt_order_fill".to_string(),
    };
    let second = BoltV3SubmitPositionSizingFillUpdate {
        trade_id: "trade-2".to_string(),
        fill_quantity: Decimal::new(3, 0),
        ..first.clone()
    };

    let first_decision = admission.apply_position_sizing_fill_update(first, 1_100);
    let second_decision = admission.apply_position_sizing_fill_update(second, 1_100);
    assert!(first_decision.accepted);
    assert_eq!(first_decision.action, PositionSizingLifecycleAction::Revalued);
    assert!(second_decision.accepted);
    assert_eq!(second_decision.action, PositionSizingLifecycleAction::Revalued);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(15, 1))
    );
}
```

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission configured_submit_sizer_applies_same_timestamp_fills_in_order -- --nocapture
```

Expected before implementation: FAIL because the lower-level reservation ledger rejects lifecycle updates with timestamps less than or equal to the current reservation timestamp.

- [ ] **Step 2: GREEN same-timestamp fills use monotonic lifecycle timestamps**

Implement the `last_lifecycle_observed_at_ns` logic from Task 1 Step 3. The metadata stores the internal ledger timestamp used for the prior reservation lifecycle mutation. It does not replace the raw NT event timestamp in logs or tests; it only satisfies the reservation ledger's strict monotonic mutation invariant.

- [ ] **Step 3: RED duplicate fill does not double-release liability**

Add:

```rust
#[test]
fn configured_submit_sizer_ignores_duplicate_fill_trade_id() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let fill = BoltV3SubmitPositionSizingFillUpdate {
        client_order_id: "client-order-1".to_string(),
        trade_id: "trade-1".to_string(),
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        side: BoltV3CompiledOrderSide::Buy,
        fill_quantity: Decimal::new(4, 0),
        observed_at_ns: 1_100,
        evidence_label: "nt_order_fill".to_string(),
    };

    let first = admission.apply_position_sizing_fill_update(fill.clone(), 1_100);
    let duplicate = admission.apply_position_sizing_fill_update(fill, 1_200);
    assert!(first.accepted);
    assert_eq!(first.action, PositionSizingLifecycleAction::Revalued);
    assert!(duplicate.accepted);
    assert_eq!(duplicate.action, PositionSizingLifecycleAction::None);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}
```

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission configured_submit_sizer_ignores_duplicate_fill_trade_id -- --nocapture
```

Expected before implementation: FAIL if duplicate fill IDs are not tracked.

- [ ] **Step 4: GREEN duplicate trade IDs are accepted no-ops**

In `apply_position_sizing_fill_update`, check `seen_trade_ids` after validating blank trade ID, non-positive fill quantity, instrument mismatch, and side mismatch, but before calculating residual liability:

```rust
if metadata.seen_trade_ids.contains(&update.trade_id) {
    return BoltV3SubmitPositionSizingLifecycleDecision {
        accepted: true,
        unknown_reservation: false,
        action: PositionSizingLifecycleAction::None,
    };
}
```

Do not update `observed_at_ns`, `filled_quantity`, or the reservation ledger for duplicate trade IDs.

- [ ] **Step 5: RED full fill releases reservation**

Add:

```rust
#[test]
fn configured_submit_sizer_full_fill_releases_reservation() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(10, 0),
            observed_at_ns: 1_100,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Released);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );

    let terminal = admission.apply_position_sizing_terminal_order_event(
        "client-order-1".to_string(),
        1_200,
        "nt_order_terminal".to_string(),
    );
    assert!(terminal.unknown_reservation);
    assert_eq!(terminal.action, PositionSizingLifecycleAction::None);
}
```

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission configured_submit_sizer_full_fill_releases_reservation -- --nocapture
```

Expected before implementation: FAIL because fills are not applied.

- [ ] **Step 6: RED mismatched fill is non-mutating**

Add:

```rust
#[test]
fn configured_submit_sizer_rejects_mismatched_fill_without_mutation() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-no.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(4, 0),
            observed_at_ns: 1_100,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.unknown_reservation);
    assert_eq!(decision.action, PositionSizingLifecycleAction::None);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}
```

Add a second mismatch test proving validation happens before duplicate trade-ID no-op detection:

```rust
#[test]
fn configured_submit_sizer_rejects_duplicate_trade_id_with_mismatched_content() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let valid = BoltV3SubmitPositionSizingFillUpdate {
        client_order_id: "client-order-1".to_string(),
        trade_id: "trade-1".to_string(),
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        side: BoltV3CompiledOrderSide::Buy,
        fill_quantity: Decimal::new(4, 0),
        observed_at_ns: 1_100,
        evidence_label: "nt_order_fill".to_string(),
    };
    let mismatched = BoltV3SubmitPositionSizingFillUpdate {
        instrument_id: "instrument-no.VENUE-A".to_string(),
        observed_at_ns: 1_200,
        ..valid.clone()
    };

    assert!(admission.apply_position_sizing_fill_update(valid, 1_100).accepted);
    let decision = admission.apply_position_sizing_fill_update(mismatched, 1_200);

    assert!(decision.unknown_reservation);
    assert_eq!(decision.action, PositionSizingLifecycleAction::None);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}
```

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission configured_submit_sizer_ -- --nocapture
```

Expected before implementation: FAIL because the fill API does not exist.

- [ ] **Step 7: RED sell-side partial fill uses sell liability factor**

Add:

```rust
#[test]
fn configured_submit_sizer_revalues_sell_residual_liability_from_fill_metadata() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    let mut request = sized_submit_request("client-order-1");
    request
        .position_sizing
        .as_mut()
        .expect("sized request should carry evidence")
        .side = BoltV3CompiledOrderSide::Sell;
    admission
        .admit_at(&request, 1_000)
        .expect("fresh sell sizing state and capacity should admit")
        .commit_submitted();
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(63, 1))
    );

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Sell,
            fill_quantity: Decimal::new(4, 0),
            observed_at_ns: 1_100,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Revalued);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(39, 1))
    );
}
```

- [ ] **Step 8: RED rebuilt reservations without submit metadata stay fail-closed**

Add:

```rust
#[test]
fn configured_submit_sizer_rejects_fill_for_rebuilt_reservation_without_metadata() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    let rebuild = admission.rebuild_position_sizing_open_order_reservations(
        vec![open_order_reservation(
            "client-order-1",
            "submit-reservation-1",
            Decimal::new(43, 1),
        )],
        1_000,
    );
    assert!(rebuild.accepted);

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(4, 0),
            observed_at_ns: 1_100,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.unknown_reservation);
    assert_eq!(decision.action, PositionSizingLifecycleAction::None);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}
```

- [ ] **Step 9: RED overfill clamps to full release with explicit evidence label**

Add:

```rust
#[test]
fn configured_submit_sizer_clamps_overfill_to_full_release() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(12, 0),
            observed_at_ns: 1_100,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Released);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}
```

The implementation should use `"nt_order_fill_clamped"` when `filled_quantity + fill_quantity > original_quantity`; this makes the silent-clamp path source-fence-visible.

- [ ] **Step 10: Verify direct fill lifecycle set**

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission configured_submit_sizer_ -- --nocapture
```

Expected after implementation: PASS.

---

## Task 3: Runtime Feed Consumes NT Fill Events

**Files:**
- Modify: `tests/bolt_v3_position_sizer_runtime_feed.rs`
- Modify: `src/bolt_v3_position_sizer_runtime_feed.rs`

- [ ] **Step 1: RED partial fill event revalues admission ledger**

Replace `partial_fill_without_liability_metadata_does_not_revalue_reservation` with:

```rust
#[test]
fn partial_fill_event_revalues_residual_reservation() {
    let admission = Arc::new(position_sized_admission());
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let mut feed = PositionSizerRuntimeFeed::new(runtime_feed_config(), admission.clone());
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        account_id(),
    )));
    let state = admission
        .position_sizer_state_snapshot()
        .expect("accepted order should publish lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 1);

    let decision = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            account_id(),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("matching fill should update residual liability");

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Revalued);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
    let state = admission
        .position_sizer_state_snapshot()
        .expect("partial fill should keep live lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 1);
}
```

Add helper `order_filled_event_with(...)` by generalizing the current `order_filled_event(...)` helper. Also import `PositionSizingLifecycleAction` from `bolt_v3_position_sizer` in this test module.
Use the existing runtime-feed test helper `sized_submit_request(...)`, whose `instrument_id` is `instrument-yes.VENUE-A`, so the submitted reservation matches `runtime_feed_config()` and the fill instrument.

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed partial_fill_event_revalues_residual_reservation -- --nocapture
```

Expected before implementation: FAIL because `OrderFilled` currently returns `None`.

- [ ] **Step 2: GREEN map NT fill event to admission fill update**

In `PositionSizerRuntimeFeed::on_order_event`, replace the current early return for `OrderEventAny::Filled(_)` with a call to a private helper:

```rust
if let OrderEventAny::Filled(fill) = event {
    return self.on_fill_event(fill);
}
```

The helper must:

- require `fill.account_id == self.config.account_id`;
- require the instrument ID to match either configured YES or configured NO instrument;
- map `OrderSide::Buy` to `BoltV3CompiledOrderSide::Buy`;
- map `OrderSide::Sell` to `BoltV3CompiledOrderSide::Sell`;
- return `None` for all other sides;
- call `submit_admission.apply_position_sizing_fill_update(...)`;
- return `None` if the admission decision is unknown.

Use evidence label `"nt_order_fill"` in the runtime feed. Admission must override the lifecycle evidence label to `"nt_order_fill_clamped"` when it detects `filled_quantity + fill_quantity > original_quantity`.

- [ ] **Step 3: RED full fill removes open order count**

Add:

```rust
#[test]
fn full_fill_event_releases_reservation_and_closes_live_order_count() {
    let admission = Arc::new(position_sized_admission());
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let mut feed = PositionSizerRuntimeFeed::new(runtime_feed_config(), admission.clone());
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        account_id(),
    )));
    let state = admission
        .position_sizer_state_snapshot()
        .expect("accepted order should publish lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 1);

    let decision = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            account_id(),
            Quantity::from(10),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("matching full fill should release reservation");

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Released);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    let state = admission
        .position_sizer_state_snapshot()
        .expect("full fill should publish lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
}
```

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed full_fill_event_releases_reservation_and_closes_live_order_count -- --nocapture
```

Expected before implementation: FAIL until the feed records full-fill terminal lifecycle.

- [ ] **Step 4: GREEN close feed live-order count only on accepted full fill**

Use the `BoltV3SubmitPositionSizingLifecycleDecision.action` field added in Task 1. In the runtime fill helper, when the returned action is `PositionSizingLifecycleAction::Released`, call:

```rust
self.component_builder
    .record_terminal_order_event(fill.client_order_id.to_string(), fill.ts_event.as_u64());
self.publish_components_if_ready();
```

Do not remove live-order count for partial-fill revalues.

- [ ] **Step 5: RED account and instrument mismatches do not mutate**

Add:

```rust
#[test]
fn fill_event_account_or_instrument_mismatch_is_non_mutating() {
    let admission = Arc::new(position_sized_admission());
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let mut feed = PositionSizerRuntimeFeed::new(runtime_feed_config(), admission.clone());
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("OTHER-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-2",
            1_200,
            account_id(),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-other.VENUE-A"),
        )))
        .is_none()
    );
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}
```

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed fill_event_account_or_instrument_mismatch_is_non_mutating -- --nocapture
```

Expected before implementation: FAIL until fill helper exists.

- [ ] **Step 6: RED duplicate fill with mismatched runtime instrument is rejected**

Add:

```rust
#[test]
fn duplicate_fill_trade_id_with_different_runtime_instrument_is_non_mutating() {
    let admission = Arc::new(position_sized_admission());
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let mut feed = PositionSizerRuntimeFeed::new(runtime_feed_config(), admission.clone());
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            account_id(),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_some()
    );
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_200,
            account_id(),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-no.VENUE-A"),
        )))
        .is_none()
    );
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}
```

- [ ] **Step 7: RED terminal event after partial fill releases residual**

Add:

```rust
#[test]
fn terminal_event_after_partial_fill_releases_residual_reservation() {
    let admission = Arc::new(position_sized_admission());
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let mut feed = PositionSizerRuntimeFeed::new(runtime_feed_config(), admission.clone());
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        account_id(),
    )));
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            account_id(),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_some()
    );

    let terminal = feed
        .on_order_event(&OrderEventAny::Canceled(order_canceled_event(
            "client-order-1",
            1_200,
        )))
        .expect("terminal after partial fill should release residual");

    assert_eq!(terminal.action, PositionSizingLifecycleAction::Released);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    let state = admission
        .position_sizer_state_snapshot()
        .expect("terminal should publish lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
}
```

- [ ] **Step 8: Verify runtime feed fill tests**

Run:

```bash
cargo test --locked --test bolt_v3_position_sizer_runtime_feed -- --nocapture
```

Expected after implementation: PASS.

---

## Task 4: Docs, Runtime Literals, And Focused Verification

**Files:**
- Modify: `specs/506-nt-position-sizer-submit/spec.md`
- Modify: `specs/506-nt-position-sizer-submit/tasks.md`
- Modify: `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`

- [ ] **Step 1: Update docs without production overclaim**

In `specs/506-nt-position-sizer-submit/spec.md`, move only this item out of the remaining production list:

```markdown
- residual liability from partial fills is revalued from authoritative NT fill events using Bolt-owned submit-time liability metadata for orders admitted after this process starts;
```

Keep startup/import attribution as remaining:

```markdown
- non-empty pre-existing NT/exchange open orders are rebuilt only after durable Bolt reservation metadata can attribute their liability;
```

In `specs/506-nt-position-sizer-submit/tasks.md`, mark residual partial-fill revalue complete and preserve the remaining unchecked items.

- [ ] **Step 2: Classify runtime literals**

Add allowed entries for:

```toml
"nt_order_fill"
"nt_order_fill_clamped"
```

If new test-only strings trigger source-fence, classify them in the existing test context rather than weakening runtime checks.

- [ ] **Step 3: Focused local verification**

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission configured_submit_sizer_ -- --nocapture
cargo test --locked --test bolt_v3_position_sizer_runtime_feed -- --nocapture
cargo test --locked --test bolt_v3_submit_admission -- --nocapture
cargo test --locked --test bolt_v3_position_sizer_runtime_feed -- --nocapture
cargo fmt --check
just source-fence
```

Expected after implementation: all pass.

- [ ] **Step 4: Commit once**

Run:

```bash
git status --short
git add src/bolt_v3_submit_admission.rs src/bolt_v3_position_sizer_runtime_feed.rs tests/bolt_v3_submit_admission.rs tests/bolt_v3_position_sizer_runtime_feed.rs specs/506-nt-position-sizer-submit/spec.md specs/506-nt-position-sizer-submit/tasks.md docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml docs/superpowers/plans/2026-06-01-position-sizer-fill-lifecycle-slice.md
git commit -m "feat: revalue position sizer reservations from fills"
```

Expected: one focused commit.

- [ ] **Step 5: Push once and use CI as source of truth**

Run:

```bash
git push origin codex/nt-position-sizer-production-slice
PR_NUMBER=$(gh pr view --json number -q .number)
gh pr checks "$PR_NUMBER" --watch --interval 10
```

Expected: CI green at the new current PR head.

- [ ] **Step 6: External review after green CI**

Request Claude, Gemini, and GLM review of the exact new diff from `2dede6baa1a36b5ed15a11112f88b265b627383f` to the new head. Do not count a failed or stuck review slot as approval. Do not wait on less trustworthy providers if Claude/Gemini/GLM provide enough signal or a provider is blocked.

---

## Production Gaps Remaining After This Plan

- Durable restart attribution for non-empty NT/exchange open orders.
- Safe replace/amend reservation transitions before enabling `ReplaceSubmit`.
- Maker quote-set reservation of simultaneous adverse fills.
- Dynamic sell-side residual repricing; this slice freezes the submit-time liability factor until maker/replace semantics define a wider revalue model.
- Conditional-token allowance evidence.
- Collateral spendability and venue/instrument allowance evidence separate from NT account free balance.
- Dynamic market metadata from NT/market-selection state.
- Cancel/flatten/halt operations tied to loss governor and sizer failures.
- Calculators for leveraged spot, futures/perps, and options.
- Live reconnect/integration proof against NT cache and event replay.

## Self-Review

- Spec coverage: this plan covers only residual partial-fill revalue from the current production-gap list and leaves the remaining gaps explicitly listed.
- Placeholder scan: no implementation step contains placeholder text; every code-changing step identifies files, behavior, command, and expected result.
- Type consistency: fill update fields use existing `BoltV3CompiledOrderSide`, `Decimal`, and NT order event identifiers; runtime feed remains a translator while admission owns ledger mutation.
