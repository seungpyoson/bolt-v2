use std::collections::BTreeMap;

use bolt_v2::bolt_v3_providers::polymarket::{
    PolymarketVenueTruthInput, PolymarketVenueTruthOrderEventMapper,
    build_polymarket_venue_truth_snapshot, extract_polymarket_token_id,
};
use bolt_v2::bolt_v3_venue_truth::{
    VenueTruthCaptureEndpointError, VenueTruthDivergenceAlarmClass, VenueTruthDivergenceKind,
    VenueTruthOpenOrder, VenueTruthOrderEventMapper, VenueTruthReconciler,
    VenueTruthReconciliation, VenueTruthSnapshot, venue_truth_capture_failure_parts,
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    enums::{LiquiditySide, OrderSide, OrderType},
    events::{OrderAccepted, OrderDenied, OrderEventAny, OrderFilled},
    identifiers::{
        AccountId, ClientOrderId, InstrumentId, StrategyId, TradeId, TraderId, VenueOrderId,
    },
    types::{Currency, Money, Price, Quantity},
};
use nautilus_polymarket::{
    common::enums::{
        PolymarketOrderSide, PolymarketOrderStatus, PolymarketOrderType, PolymarketOutcome,
    },
    http::{
        models::{DataApiPosition, PolymarketOpenOrder},
        query::BalanceAllowance,
    },
};
use rust_decimal::Decimal;
use ustr::Ustr;

#[test]
fn extracts_polymarket_token_id_from_symbol_suffix() {
    let instrument_id = InstrumentId::from("condition-with-dash-token123.POLYMARKET");

    assert_eq!(
        extract_polymarket_token_id(&instrument_id),
        Some("token123".to_string())
    );
}

#[test]
fn rejects_non_polymarket_token_id_sources() {
    assert_eq!(
        extract_polymarket_token_id(&InstrumentId::from("condition-token123.SOURCE")),
        None
    );
    assert_eq!(
        extract_polymarket_token_id(&InstrumentId::from("conditionwithouttoken.POLYMARKET")),
        None
    );
}

#[test]
fn builds_snapshot_from_balance_orders_and_positions() {
    let snapshot = build_polymarket_venue_truth_snapshot(PolymarketVenueTruthInput {
        captured_at: UnixNanos::from(1_000),
        account_id: AccountId::from("POLYMARKET-001"),
        collateral_currency: Currency::pUSD(),
        collateral: BalanceAllowance {
            balance: Decimal::new(50_000_000, 0),
            allowance: Some(Decimal::new(40_000_000, 0)),
        },
        open_orders: vec![open_order(
            "venue-order-1",
            "condition-token123",
            "token123",
            Decimal::new(10, 0),
            Decimal::new(4, 0),
        )],
        positions: vec![DataApiPosition {
            asset: "token123".to_string(),
            condition_id: "condition".to_string(),
            size: 6.5,
            avg_price: Some(0.42),
        }],
    })
    .expect("valid venue truth input should convert");

    assert_eq!(snapshot.captured_at, UnixNanos::from(1_000));
    assert_eq!(snapshot.account_id, AccountId::from("POLYMARKET-001"));
    assert_eq!(
        snapshot.collateral_balance,
        Money::from_decimal(Decimal::new(5000, 2), Currency::pUSD()).unwrap()
    );
    assert_eq!(
        snapshot.collateral_allowance,
        Money::from_decimal(Decimal::new(4000, 2), Currency::pUSD()).unwrap()
    );
    assert_eq!(
        snapshot.open_orders,
        BTreeMap::from([(
            VenueOrderId::from("venue-order-1"),
            VenueTruthOpenOrder {
                venue_order_id: VenueOrderId::from("venue-order-1"),
                market_id: "condition-token123".to_string(),
                product_id: "token123".to_string(),
                side: OrderSide::Buy,
                original_size: Decimal::new(10, 0),
                size_matched: Decimal::new(4, 0),
                open_size: Decimal::new(6, 0),
                price: Decimal::new(42, 2),
            },
        )])
    );
    assert_eq!(
        snapshot.positions_by_product_id,
        BTreeMap::from([("token123".to_string(), Decimal::new(65, 1))])
    );
}

#[test]
fn capture_failure_parts_survive_anyhow_context() {
    let error = anyhow::anyhow!(VenueTruthCaptureEndpointError::new(
        "clob_balance_allowance",
        "transport_or_decode",
        anyhow::anyhow!("synthetic endpoint failure"),
    ))
    .context("poll Polymarket venue-truth endpoints");

    assert_eq!(
        venue_truth_capture_failure_parts(&error),
        ("clob_balance_allowance", "transport_or_decode")
    );
}

#[test]
fn accepted_order_event_explains_new_venue_open_order() {
    let mut reconciler = VenueTruthReconciler::new();

    assert_eq!(
        reconciler
            .reconcile_snapshot(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
            .expect("initial venue truth establishes the baseline"),
        VenueTruthReconciliation::BaselineAccepted
    );

    record_order_event(
        &mut reconciler,
        OrderEventAny::Accepted(order_accepted_event(
            "client-order-1",
            "venue-order-1",
            "condition-token123.POLYMARKET",
            1_100,
        )),
    );

    assert_eq!(
        reconciler
            .reconcile_snapshot(snapshot_with_order(
                1_200,
                Decimal::new(50_000_000, 0),
                Decimal::ZERO,
                0.0,
            ))
            .expect("accepted order should explain the venue order appearance"),
        VenueTruthReconciliation::DeltaExplained
    );
}

#[test]
fn accepted_order_event_does_not_explain_collateral_delta_without_fill() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .reconcile_snapshot(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("initial venue truth establishes the baseline");
    record_order_event(
        &mut reconciler,
        OrderEventAny::Accepted(order_accepted_event(
            "client-order-1",
            "venue-order-1",
            "condition-token123.POLYMARKET",
            1_100,
        )),
    );

    assert_eq!(
        reconciler
            .reconcile_snapshot(snapshot_with_order(
                1_200,
                Decimal::new(51_000_000, 0),
                Decimal::ZERO,
                0.0,
            ))
            .expect("unexplained collateral movement should pend until its capture fence"),
        VenueTruthReconciliation::DeltaPending
    );

    let divergence = reconciler
        .reconcile_snapshot(snapshot_with_order(
            1_300,
            Decimal::new(51_000_000, 0),
            Decimal::ZERO,
            0.0,
        ))
        .expect_err("accepted order alone must not explain collateral movement at fence");

    assert_eq!(
        divergence.kind,
        VenueTruthDivergenceKind::UnexplainedCollateralDelta
    );
}

#[test]
fn filled_order_event_explains_open_order_and_position_delta() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .reconcile_snapshot(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("initial venue truth establishes the baseline");
    record_order_event(
        &mut reconciler,
        OrderEventAny::Accepted(order_accepted_event(
            "client-order-1",
            "venue-order-1",
            "condition-token123.POLYMARKET",
            1_100,
        )),
    );
    reconciler
        .reconcile_snapshot(snapshot_with_order(
            1_200,
            Decimal::new(50_000_000, 0),
            Decimal::ZERO,
            0.0,
        ))
        .expect("accepted order should explain the venue order appearance");

    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_300,
        )),
    );

    assert_eq!(
        reconciler
            .reconcile_snapshot(snapshot_with_order(
                1_400,
                Decimal::new(48_400_000, 0),
                Decimal::new(4, 0),
                4.0,
            ))
            .expect("fill should explain position and open-order matched deltas"),
        VenueTruthReconciliation::DeltaExplained
    );
}

#[test]
fn filled_order_event_uses_actual_fill_price_and_fee_for_collateral() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .reconcile_snapshot(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("initial venue truth establishes the baseline");
    record_order_event(
        &mut reconciler,
        OrderEventAny::Accepted(order_accepted_event(
            "client-order-1",
            "venue-order-1",
            "condition-token123.POLYMARKET",
            1_100,
        )),
    );
    reconciler
        .reconcile_snapshot(snapshot_with_order(
            1_200,
            Decimal::new(50_000_000, 0),
            Decimal::ZERO,
            0.0,
        ))
        .expect("accepted order should explain the venue order appearance");

    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event_with_price_and_fee(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            Price::from("0.40"),
            Money::from_decimal(Decimal::new(1, 2), Currency::pUSD())
                .expect("fee money should construct"),
            1_300,
        )),
    );

    assert_eq!(
        reconciler
            .reconcile_snapshot(snapshot_with_order(
                1_400,
                Decimal::new(48_390_000, 0),
                Decimal::new(4, 0),
                4.0,
            ))
            .expect("actual fill price plus explicit fee should explain collateral"),
        VenueTruthReconciliation::DeltaExplained
    );
}

#[test]
fn allowance_decrease_is_explained_by_consumed_fills() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .reconcile_snapshot(empty_snapshot_with_allowance(
            1_000,
            Decimal::new(50_000_000, 0),
            Decimal::new(40_000_000, 0),
        ))
        .expect("initial venue truth establishes the baseline");

    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_100,
        )),
    );

    assert_eq!(
        reconciler
            .reconcile_snapshot(snapshot_with_position_and_allowance(
                1_200,
                Decimal::new(48_400_000, 0),
                Decimal::new(38_400_000, 0),
                4.0,
            ))
            .expect("finite allowance decrease should be explained by the consumed buy fill"),
        VenueTruthReconciliation::DeltaExplained
    );
}

#[test]
fn allowance_increase_is_unexplained_and_halts_at_fence() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .reconcile_snapshot(empty_snapshot_with_allowance(
            1_000,
            Decimal::new(50_000_000, 0),
            Decimal::new(40_000_000, 0),
        ))
        .expect("initial venue truth establishes the baseline");

    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_100,
        )),
    );

    assert_eq!(
        reconciler
            .record_snapshot_completion(snapshot_with_position_and_allowance(
                1_200,
                Decimal::new(48_400_000, 0),
                Decimal::new(41_000_000, 0),
                4.0,
            ))
            .expect("allowance top-up should pend until the capture fence")[0]
            .outcome,
        VenueTruthReconciliation::DeltaPending
    );

    let divergence = reconciler
        .record_snapshot_completion(snapshot_with_position_and_allowance(
            1_300,
            Decimal::new(48_400_000, 0),
            Decimal::new(41_000_000, 0),
            4.0,
        ))
        .expect_err("allowance top-up must halt loudly at its fence");

    assert_eq!(
        divergence.kind,
        VenueTruthDivergenceKind::UnexplainedCollateralDelta
    );
    assert_eq!(divergence.field, "collateral_allowance");
}

#[test]
fn zero_allowance_delta_after_fill_is_no_op() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .reconcile_snapshot(empty_snapshot_with_allowance(
            1_000,
            Decimal::new(50_000_000, 0),
            Decimal::new(40_000_000, 0),
        ))
        .expect("initial venue truth establishes the baseline");

    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_100,
        )),
    );

    assert_eq!(
        reconciler
            .reconcile_snapshot(snapshot_with_position_and_allowance(
                1_200,
                Decimal::new(48_400_000, 0),
                Decimal::new(40_000_000, 0),
                4.0,
            ))
            .expect("infinite-approval behavior leaves allowance unchanged"),
        VenueTruthReconciliation::DeltaExplained
    );
}

#[test]
fn taker_fill_without_resting_open_order_explains_position_and_collateral() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .reconcile_snapshot(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("initial venue truth establishes the baseline");

    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_100,
        )),
    );

    assert_eq!(
        reconciler
            .reconcile_snapshot(snapshot_with_position(
                1_200,
                Decimal::new(48_400_000, 0),
                4.0,
            ))
            .expect("recorded taker fill should explain venue truth without an open-order row"),
        VenueTruthReconciliation::DeltaExplained
    );
}

#[test]
fn unexplained_capture_is_pending_until_next_capture_fence() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .record_snapshot_completion(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("baseline should be accepted");

    let results = reconciler
        .record_snapshot_completion(snapshot_with_position(
            1_100,
            Decimal::new(48_400_000, 0),
            4.0,
        ))
        .expect("unexplained delta before fence should be pending, not divergent");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, VenueTruthReconciliation::DeltaPending);
    assert_eq!(
        reconciler
            .latest_accepted_snapshot()
            .expect("baseline remains accepted")
            .captured_at,
        UnixNanos::from(1_000)
    );
}

#[test]
fn pending_capture_accepts_after_interleaved_fill_at_next_capture_fence() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .record_snapshot_completion(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("baseline should be accepted");
    reconciler
        .record_snapshot_completion(snapshot_with_position(
            1_100,
            Decimal::new(48_400_000, 0),
            4.0,
        ))
        .expect("unexplained delta before fence should be pending");
    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_150,
        )),
    );

    let results = reconciler
        .record_snapshot_completion(snapshot_with_position(
            1_200,
            Decimal::new(48_400_000, 0),
            4.0,
        ))
        .expect("interleaved fill should explain the pending delta at fence");

    assert!(
        results.iter().any(|result| result.capture_number == 2
            && result.outcome == VenueTruthReconciliation::DeltaExplained),
        "capture 2 must be accepted when its fence completes"
    );
}

#[test]
fn positions_fresher_than_balance_skew_pends_then_explains_at_fence() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .record_snapshot_completion(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("baseline should be accepted");
    for (trade_id, observed_at_ns) in [("trade-1", 1_100), ("trade-2", 1_110)] {
        record_order_event(
            &mut reconciler,
            OrderEventAny::Filled(order_filled_event(
                "client-order-1",
                "venue-order-1",
                trade_id,
                "condition-token123.POLYMARKET",
                OrderSide::Buy,
                Quantity::from("4"),
                observed_at_ns,
            )),
        );
    }

    let results = reconciler
        .record_snapshot_completion(snapshot_with_position(
            1_200,
            Decimal::new(50_000_000, 0),
            4.0,
        ))
        .expect("position-fresher-than-balance skew should pend before its fence");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, VenueTruthReconciliation::DeltaPending);

    let results = reconciler
        .record_snapshot_completion(snapshot_with_position(
            1_300,
            Decimal::new(46_800_000, 0),
            8.0,
        ))
        .expect("balance catch-up at the fence should explain the skew without durable halt");

    assert!(
        results.iter().any(|result| result.capture_number == 2
            && result.outcome == VenueTruthReconciliation::DeltaExplained),
        "capture 2 must be accepted at its fence after the balance endpoint catches up"
    );
    assert!(
        results.iter().any(|result| result.capture_number == 3
            && result.outcome == VenueTruthReconciliation::DeltaExplained),
        "capture 3 must drain the deferred collateral from the same consumed fills"
    );
}

#[test]
fn pending_capture_without_channel_event_halts_as_silent_channel_at_fence() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .record_snapshot_completion(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("baseline should be accepted");
    reconciler
        .record_snapshot_completion(snapshot_with_position(
            1_100,
            Decimal::new(48_400_000, 0),
            4.0,
        ))
        .expect("unexplained delta before fence should be pending");

    let divergence = reconciler
        .record_snapshot_completion(snapshot_with_position(
            1_200,
            Decimal::new(48_400_000, 0),
            4.0,
        ))
        .expect_err("still-unexplained pending delta should halt at its fence");

    assert_eq!(
        divergence.alarm_class,
        VenueTruthDivergenceAlarmClass::SilentChannel
    );
}

#[test]
fn same_domain_ordering_violation_halts_even_when_deltas_explain() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .record_snapshot_completion(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("baseline should be accepted");
    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_300,
        )),
    );
    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-2",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_200,
        )),
    );

    let divergence = reconciler
        .record_snapshot_completion(snapshot_with_position(
            1_400,
            Decimal::new(46_800_000, 0),
            8.0,
        ))
        .expect_err("same-domain timestamp regression must halt even with explained deltas");

    assert_eq!(
        divergence.alarm_class,
        VenueTruthDivergenceAlarmClass::OrderingViolation
    );
}

#[test]
fn local_denied_terminal_timestamp_does_not_create_venue_ordering_violation() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .record_snapshot_completion(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("baseline should be accepted");
    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_300,
        )),
    );
    record_order_event(
        &mut reconciler,
        OrderEventAny::Denied(order_denied_event(
            "client-order-2",
            "condition-token123.POLYMARKET",
            1_200,
        )),
    );

    assert_eq!(
        reconciler
            .record_snapshot_completion(snapshot_with_position(
                1_400,
                Decimal::new(48_400_000, 0),
                4.0,
            ))
            .expect("local terminal timestamp must not poison venue-domain ordering")[0]
            .outcome,
        VenueTruthReconciliation::DeltaExplained
    );
}

#[test]
fn slow_reconcile_consumes_interleaved_events_when_fence_already_completed() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .record_snapshot_completion(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("baseline should be accepted");
    reconciler.record_snapshot_completion_without_processing(snapshot_with_position(
        1_100,
        Decimal::new(48_400_000, 0),
        4.0,
    ));
    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_150,
        )),
    );
    reconciler.record_snapshot_completion_without_processing(snapshot_with_position(
        1_200,
        Decimal::new(48_400_000, 0),
        4.0,
    ));

    let results = reconciler
        .process_completed_captures()
        .expect("fence already complete should re-judge once after interleaved events");

    assert!(
        results.iter().any(|result| result.capture_number == 2
            && result.outcome == VenueTruthReconciliation::DeltaExplained),
        "capture 2 must consume the fill recorded between capture completions"
    );
}

#[test]
fn filled_order_event_does_not_explain_unrelated_collateral_delta() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .reconcile_snapshot(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("initial venue truth establishes the baseline");
    record_order_event(
        &mut reconciler,
        OrderEventAny::Accepted(order_accepted_event(
            "client-order-1",
            "venue-order-1",
            "condition-token123.POLYMARKET",
            1_100,
        )),
    );
    reconciler
        .reconcile_snapshot(snapshot_with_order(
            1_200,
            Decimal::new(50_000_000, 0),
            Decimal::ZERO,
            0.0,
        ))
        .expect("accepted order should explain the venue order appearance");

    record_order_event(
        &mut reconciler,
        OrderEventAny::Filled(order_filled_event(
            "client-order-1",
            "venue-order-1",
            "trade-1",
            "condition-token123.POLYMARKET",
            OrderSide::Buy,
            Quantity::from("4"),
            1_300,
        )),
    );

    assert_eq!(
        reconciler
            .reconcile_snapshot(snapshot_with_order(
                1_400,
                Decimal::new(51_000_000, 0),
                Decimal::new(4, 0),
                4.0,
            ))
            .expect("unrelated collateral movement should pend until its capture fence"),
        VenueTruthReconciliation::DeltaPending
    );

    let divergence = reconciler
        .reconcile_snapshot(snapshot_with_order(
            1_500,
            Decimal::new(51_000_000, 0),
            Decimal::new(4, 0),
            4.0,
        ))
        .expect_err("unrelated collateral movement must not be explained by a valid fill at fence");

    assert_eq!(
        divergence.kind,
        VenueTruthDivergenceKind::UnexplainedCollateralDelta
    );
}

#[test]
fn collateral_only_operator_transfer_is_unexplainable() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .reconcile_snapshot(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("initial venue truth establishes the baseline");

    assert_eq!(
        reconciler
            .reconcile_snapshot(empty_snapshot(1_100, Decimal::new(51_000_000, 0)))
            .expect("manual transfer should pend until its capture fence"),
        VenueTruthReconciliation::DeltaPending
    );

    let divergence = reconciler
        .reconcile_snapshot(empty_snapshot(1_200, Decimal::new(51_000_000, 0)))
        .expect_err("manual transfer should not be explainable by order events at fence");

    assert_eq!(
        divergence.kind,
        VenueTruthDivergenceKind::UnexplainedCollateralDelta
    );
}

fn record_order_event(reconciler: &mut VenueTruthReconciler, event: OrderEventAny) {
    let mapper = PolymarketVenueTruthOrderEventMapper;
    let mapped = mapper
        .map_order_event(&event)
        .expect("test order event should map into venue truth projection");
    reconciler.record_order_event(mapped);
}

fn open_order(
    id: &str,
    market: &str,
    asset_id: &str,
    original_size: Decimal,
    size_matched: Decimal,
) -> PolymarketOpenOrder {
    PolymarketOpenOrder {
        associate_trades: None,
        id: id.to_string(),
        status: PolymarketOrderStatus::Live,
        market: Ustr::from(market),
        original_size,
        outcome: PolymarketOutcome::yes(),
        maker_address: "maker".to_string(),
        owner: "owner".to_string(),
        price: Decimal::new(42, 2),
        side: PolymarketOrderSide::Buy,
        size_matched,
        asset_id: Ustr::from(asset_id),
        expiration: None,
        order_type: PolymarketOrderType::GTC,
        created_at: 1_000,
    }
}

fn empty_snapshot(captured_at: u64, collateral_balance: Decimal) -> VenueTruthSnapshot {
    empty_snapshot_with_allowance(captured_at, collateral_balance, Decimal::new(40_000_000, 0))
}

fn empty_snapshot_with_allowance(
    captured_at: u64,
    collateral_balance: Decimal,
    collateral_allowance: Decimal,
) -> VenueTruthSnapshot {
    build_polymarket_venue_truth_snapshot(PolymarketVenueTruthInput {
        captured_at: UnixNanos::from(captured_at),
        account_id: AccountId::from("POLYMARKET-001"),
        collateral_currency: Currency::pUSD(),
        collateral: BalanceAllowance {
            balance: collateral_balance,
            allowance: Some(collateral_allowance),
        },
        open_orders: Vec::new(),
        positions: Vec::new(),
    })
    .expect("test snapshot should be valid")
}

fn snapshot_with_order(
    captured_at: u64,
    collateral_balance: Decimal,
    size_matched: Decimal,
    venue_position_quantity: f64,
) -> VenueTruthSnapshot {
    build_polymarket_venue_truth_snapshot(PolymarketVenueTruthInput {
        captured_at: UnixNanos::from(captured_at),
        account_id: AccountId::from("POLYMARKET-001"),
        collateral_currency: Currency::pUSD(),
        collateral: BalanceAllowance {
            balance: collateral_balance,
            allowance: Some(Decimal::new(40_000_000, 0)),
        },
        open_orders: vec![open_order(
            "venue-order-1",
            "condition-token123",
            "token123",
            Decimal::new(10, 0),
            size_matched,
        )],
        positions: vec![DataApiPosition {
            asset: "token123".to_string(),
            condition_id: "condition".to_string(),
            size: venue_position_quantity,
            avg_price: Some(0.42),
        }],
    })
    .expect("test snapshot should be valid")
}

fn snapshot_with_position(
    captured_at: u64,
    collateral_balance: Decimal,
    venue_position_quantity: f64,
) -> VenueTruthSnapshot {
    snapshot_with_position_and_allowance(
        captured_at,
        collateral_balance,
        Decimal::new(40_000_000, 0),
        venue_position_quantity,
    )
}

fn snapshot_with_position_and_allowance(
    captured_at: u64,
    collateral_balance: Decimal,
    collateral_allowance: Decimal,
    venue_position_quantity: f64,
) -> VenueTruthSnapshot {
    build_polymarket_venue_truth_snapshot(PolymarketVenueTruthInput {
        captured_at: UnixNanos::from(captured_at),
        account_id: AccountId::from("POLYMARKET-001"),
        collateral_currency: Currency::pUSD(),
        collateral: BalanceAllowance {
            balance: collateral_balance,
            allowance: Some(collateral_allowance),
        },
        open_orders: Vec::new(),
        positions: vec![DataApiPosition {
            asset: "token123".to_string(),
            condition_id: "condition".to_string(),
            size: venue_position_quantity,
            avg_price: Some(0.42),
        }],
    })
    .expect("test snapshot should be valid")
}

fn order_accepted_event(
    client_order_id: &str,
    venue_order_id: &str,
    instrument_id: &str,
    ts_event: u64,
) -> OrderAccepted {
    OrderAccepted::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from(instrument_id),
        ClientOrderId::from(client_order_id),
        VenueOrderId::from(venue_order_id),
        AccountId::from("POLYMARKET-001"),
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
    )
}

fn order_filled_event(
    client_order_id: &str,
    venue_order_id: &str,
    trade_id: &str,
    instrument_id: &str,
    order_side: OrderSide,
    quantity: Quantity,
    ts_event: u64,
) -> OrderFilled {
    order_filled_event_with_price_and_optional_fee(
        client_order_id,
        venue_order_id,
        trade_id,
        instrument_id,
        order_side,
        quantity,
        Price::from("0.40"),
        None,
        ts_event,
    )
}

#[expect(clippy::too_many_arguments)]
fn order_filled_event_with_price_and_fee(
    client_order_id: &str,
    venue_order_id: &str,
    trade_id: &str,
    instrument_id: &str,
    order_side: OrderSide,
    quantity: Quantity,
    price: Price,
    fee: Money,
    ts_event: u64,
) -> OrderFilled {
    order_filled_event_with_price_and_optional_fee(
        client_order_id,
        venue_order_id,
        trade_id,
        instrument_id,
        order_side,
        quantity,
        price,
        Some(fee),
        ts_event,
    )
}

#[expect(clippy::too_many_arguments)]
fn order_filled_event_with_price_and_optional_fee(
    client_order_id: &str,
    venue_order_id: &str,
    trade_id: &str,
    instrument_id: &str,
    order_side: OrderSide,
    quantity: Quantity,
    price: Price,
    fee: Option<Money>,
    ts_event: u64,
) -> OrderFilled {
    OrderFilled::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from(instrument_id),
        ClientOrderId::from(client_order_id),
        VenueOrderId::from(venue_order_id),
        AccountId::from("POLYMARKET-001"),
        TradeId::from(trade_id),
        order_side,
        OrderType::Limit,
        quantity,
        price,
        Currency::pUSD(),
        LiquiditySide::Taker,
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
        None,
        fee,
    )
}

fn order_denied_event(client_order_id: &str, instrument_id: &str, ts_event: u64) -> OrderDenied {
    OrderDenied::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from(instrument_id),
        ClientOrderId::from(client_order_id),
        Ustr::from("test-denied"),
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
    )
}
