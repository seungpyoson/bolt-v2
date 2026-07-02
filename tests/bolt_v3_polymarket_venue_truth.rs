use std::collections::BTreeMap;

use bolt_v2::bolt_v3_providers::polymarket::{
    PolymarketVenueTruthInput, PolymarketVenueTruthOrderEventMapper,
    build_polymarket_venue_truth_snapshot, extract_polymarket_token_id,
};
use bolt_v2::bolt_v3_venue_truth::{
    VenueTruthDivergenceKind, VenueTruthOpenOrder, VenueTruthOrderEventMapper,
    VenueTruthReconciler, VenueTruthReconciliation, VenueTruthSnapshot,
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    enums::{LiquiditySide, OrderSide, OrderType},
    events::{OrderAccepted, OrderEventAny, OrderFilled},
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

    let divergence = reconciler
        .reconcile_snapshot(snapshot_with_order(
            1_200,
            Decimal::new(51_000_000, 0),
            Decimal::ZERO,
            0.0,
        ))
        .expect_err("accepted order alone must not explain collateral movement");

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
                Decimal::new(48_320_000, 0),
                Decimal::new(4, 0),
                4.0,
            ))
            .expect("fill should explain position and open-order matched deltas"),
        VenueTruthReconciliation::DeltaExplained
    );
}

#[test]
fn collateral_only_operator_transfer_is_unexplainable() {
    let mut reconciler = VenueTruthReconciler::new();
    reconciler
        .reconcile_snapshot(empty_snapshot(1_000, Decimal::new(50_000_000, 0)))
        .expect("initial venue truth establishes the baseline");

    let divergence = reconciler
        .reconcile_snapshot(empty_snapshot(1_100, Decimal::new(51_000_000, 0)))
        .expect_err("manual transfer should not be explainable by order events");

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
    build_polymarket_venue_truth_snapshot(PolymarketVenueTruthInput {
        captured_at: UnixNanos::from(captured_at),
        account_id: AccountId::from("POLYMARKET-001"),
        collateral_currency: Currency::pUSD(),
        collateral: BalanceAllowance {
            balance: collateral_balance,
            allowance: Some(Decimal::new(40_000_000, 0)),
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
        Price::from("0.40"),
        Currency::pUSD(),
        LiquiditySide::Taker,
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
        None,
        None,
    )
}
