//! What the pinned NautilusTrader engine does with the venue order reports Bolt
//! actually sees, asserted against the pinned dependency rather than described.
//!
//! Bolt runs the execution engine with no cache database
//! (`bolt_v3_live_node::live_node_config` sets `cache: None`) and requires
//! `filter_unclaimed_external_orders = false`. At startup reconciliation the
//! engine's order cache is therefore empty and every venue order report is
//! unknown to it. Unknown reports do not reach the cached-order path that
//! computes `requires_snapshot_projection`; they reach `handle_external_order`,
//! which synthesises an order from the report and calls
//! [`generate_external_order_status_events`].
//!
//! The `reconciliation_unmet` condition covering the adapter's silent discard of
//! non-confirmed trades turns on what that function does. Four review rounds
//! described it in prose and got it wrong four different ways, because prose
//! about a dependency's internals has nothing to fail against -- the assertion
//! that replaced it last round checked that Bolt's own condition string
//! contained the words "canceled or expired", so it stayed green while the
//! claim those words made was false.
//!
//! These tests assert the dependency's behaviour instead. A wrong description
//! now fails a build, and a pin bump that changes the behaviour fails here
//! rather than silently invalidating the gate's stated reason.

use nautilus_core::{UUID4, UnixNanos};
use nautilus_execution::reconciliation::{
    generate_external_order_status_events, reconcile_fill_report,
};
use nautilus_model::{
    enums::{
        AssetClass, ContingencyType, LiquiditySide, OrderSide, OrderStatus, OrderType, TimeInForce,
    },
    events::{OrderEventAny, OrderInitialized},
    identifiers::{
        AccountId, ClientOrderId, InstrumentId, StrategyId, Symbol, TradeId, TraderId, VenueOrderId,
    },
    instruments::{BinaryOption, InstrumentAny},
    orders::{Order, OrderAny},
    reports::{FillReport, OrderStatusReport},
    types::{Currency, Money, Price, Quantity},
};

/// The venue trade id the adapter never supplied, used to show that the id on
/// an inferred fill is not it.
const VENUE_TRADE_ID: &str = "0xfeedfacefeedfacefeedfacefeedface";

fn instrument() -> InstrumentAny {
    InstrumentAny::BinaryOption(BinaryOption::new(
        InstrumentId::from("0x1234-UP.POLYMARKET"),
        Symbol::from("0x1234-UP"),
        AssetClass::Alternative,
        Currency::from("USDC"),
        UnixNanos::default(),
        UnixNanos::from(u64::MAX),
        2,
        2,
        Price::from("0.01"),
        Quantity::from("0.01"),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

/// A venue order report as the Polymarket adapter hands it to the engine.
fn report(status: OrderStatus, filled_qty: &str) -> OrderStatusReport {
    let mut report = OrderStatusReport::new(
        AccountId::from("POLYMARKET-001"),
        InstrumentId::from("0x1234-UP.POLYMARKET"),
        None,
        VenueOrderId::from("0xabc"),
        OrderSide::Buy,
        OrderType::Limit,
        TimeInForce::Gtc,
        status,
        Quantity::from("100"),
        Quantity::from(filled_qty),
        UnixNanos::default(),
        UnixNanos::default(),
        UnixNanos::default(),
        None,
    );
    report.price = Some(Price::from("0.50"));
    report.post_only = true;
    report
}

/// Mirrors `handle_external_order`: the engine has never seen this order, so it
/// builds one from the report before generating events.
fn external_order(report: &OrderStatusReport) -> OrderAny {
    let initialized = OrderInitialized::new(
        TraderId::from("BOLT-001"),
        StrategyId::from("EXTERNAL"),
        report.instrument_id,
        ClientOrderId::from("O-EXTERNAL-1"),
        report.order_side,
        report.order_type,
        report.quantity,
        report.time_in_force,
        report.post_only,
        report.reduce_only,
        false,
        true,
        UUID4::new(),
        UnixNanos::default(),
        UnixNanos::default(),
        report.price,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(ContingencyType::NoContingency),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    OrderAny::from_events(vec![OrderEventAny::Initialized(initialized)])
        .expect("an order built from a venue report must be constructible")
}

fn events_for(status: OrderStatus, filled_qty: &str) -> Vec<OrderEventAny> {
    let report = report(status, filled_qty);
    let order = external_order(&report);
    generate_external_order_status_events(
        &order,
        &report,
        &AccountId::from("POLYMARKET-001"),
        &instrument(),
        UnixNanos::default(),
    )
}

fn fills(events: &[OrderEventAny]) -> Vec<&nautilus_model::events::OrderFilled> {
    events
        .iter()
        .filter_map(|event| match event {
            OrderEventAny::Filled(filled) => Some(filled),
            _ => None,
        })
        .collect()
}

/// While the venue still reports the order working, no fill is applied at any
/// filled quantity. This is the one clause of the condition that has been right
/// in every telling; it is pinned so that it stays right.
#[test]
fn a_working_order_report_yields_no_fill_at_any_filled_quantity() {
    let events = events_for(OrderStatus::Accepted, "40");
    assert!(
        fills(&events).is_empty(),
        "an accepted report must produce no fill regardless of filled_qty: {events:#?}"
    );
}

/// The state the pinned adapter actually produces for a settlement-pending
/// trade: its `cap_order_reports_to_confirmed_fills` floors local filled
/// quantity at zero, and a matched-but-unconfirmed trade has no confirmed fill
/// report, so the report reaching the engine carries `filled_qty == 0`.
///
/// With that quantity the terminal branch infers nothing, so the quantity is
/// never recovered and stays understated. No inferred fill exists for the
/// venue's later confirmation to collide with.
#[test]
fn a_terminal_report_whose_quantity_was_capped_to_zero_yields_no_fill() {
    for status in [OrderStatus::Canceled, OrderStatus::Expired] {
        let events = events_for(status, "0");
        assert!(
            fills(&events).is_empty(),
            "{status:?} with a zero filled quantity must infer no fill: {events:#?}"
        );
    }
}

/// The coupling between the two conditions, made executable.
///
/// The protection above is the zero quantity, *not* the terminal status. Give
/// the same branch a non-zero filled quantity -- which is exactly what closing
/// the zero-floor condition would do -- and it infers a fill, identified by an
/// id derived from the order rather than by the venue trade id the adapter
/// never supplied. NT deduplicates fills by trade-id equality
/// (`reconcile_fill_report`), so the venue's own later confirmation of that
/// same trade matches nothing and is applied on top of it.
///
/// This inverts the intuition that closing one condition can only help: closing
/// the zero floor alone turns a permanent understatement into a double count.
#[test]
fn a_terminal_report_with_quantity_infers_a_fill_the_venue_cannot_deduplicate_against() {
    for status in [OrderStatus::Canceled, OrderStatus::Expired] {
        let events = events_for(status, "40");
        let fills = fills(&events);
        assert_eq!(
            fills.len(),
            1,
            "{status:?} with a non-zero filled quantity must infer a fill: {events:#?}"
        );

        let fill = fills[0];
        assert_eq!(
            fill.last_qty,
            Quantity::from("40"),
            "the inferred fill must carry the report's filled quantity"
        );
        assert_ne!(
            fill.trade_id.to_string(),
            VENUE_TRADE_ID,
            "the inferred fill must not carry the venue trade id -- that is the \
             whole reason a later confirmation cannot be deduplicated against it"
        );
        assert_eq!(
            fill.liquidity_side,
            LiquiditySide::Maker,
            "a post-only report must infer a maker fill"
        );
    }
}

/// The other half of the collision, and the clause that has been hardest to
/// state correctly: that the venue's own later report of the same trade is not
/// recognised as a duplicate of the inferred fill.
///
/// NT deduplicates by trade-id equality, and the inferred fill's id was derived
/// from the order, so the venue's confirmation carrying the real trade id is
/// accepted and applied on top -- the same executed quantity counted twice.
///
/// Bolt configures `allow_overfills = false`, so this holds only while the two
/// together stay within the order's quantity. Past that the engine rejects the
/// venue's *real* fill and drops it instead, which is a different corruption
/// rather than a correction. Both halves are asserted.
#[test]
fn the_venues_own_later_confirmation_is_not_deduplicated_against_an_inferred_fill() {
    let status_report = report(OrderStatus::Canceled, "40");
    let mut order = external_order(&status_report);
    let events = generate_external_order_status_events(
        &order,
        &status_report,
        &AccountId::from("POLYMARKET-001"),
        &instrument(),
        UnixNanos::default(),
    );
    for event in events {
        // The terminal event may be rejected by the order state machine
        // depending on ordering; the fill is the one that must apply.
        let _ = order.apply(event);
    }
    assert_eq!(
        order.filled_qty(),
        Quantity::from("40"),
        "the inferred fill must have been applied to the order"
    );

    let confirmation = |qty: &str| {
        FillReport::new(
            AccountId::from("POLYMARKET-001"),
            InstrumentId::from("0x1234-UP.POLYMARKET"),
            VenueOrderId::from("0xabc"),
            TradeId::from(VENUE_TRADE_ID),
            OrderSide::Buy,
            Quantity::from(qty),
            Price::from("0.50"),
            Money::new(0.0, Currency::from("USDC")),
            LiquiditySide::Maker,
            None,
            None,
            UnixNanos::default(),
            UnixNanos::default(),
            None,
        )
    };

    // Applied, not merely produced. Review found this asserting only that an
    // event came back while its own message claimed the quantity was counted
    // twice -- a stronger claim than the assertion settled, and the exact shape
    // of test that guards a description instead of a dependency. The projected
    // quantity is the thing condition 3 is about, so the projection is what is
    // asserted.
    let within_quantity = reconcile_fill_report(
        &order,
        &confirmation("40"),
        &instrument(),
        UnixNanos::default(),
        false,
    )
    .expect(
        "the venue's confirmation carries the real trade id, which the inferred fill's \
         derived id does not match, so it is not deduplicated away",
    );
    let mut projected = order.clone();
    projected
        .apply(within_quantity)
        .expect("the venue's confirmation applies on top of the inferred fill");
    assert_eq!(
        projected.filled_qty(),
        Quantity::from("80"),
        "forty executed units, reported once by the inferred fill and once by the venue's \
         own confirmation, project to eighty: the same executed quantity counted twice"
    );

    let past_quantity = reconcile_fill_report(
        &order,
        &confirmation("100"),
        &instrument(),
        UnixNanos::default(),
        false,
    );
    assert!(
        past_quantity.is_none(),
        "with allow_overfills=false the engine drops the venue's real fill once \
         it would exceed the order quantity, rather than correcting the inferred one"
    );
}
