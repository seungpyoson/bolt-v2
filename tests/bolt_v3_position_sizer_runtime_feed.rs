mod support;

use std::sync::{Arc, Mutex};

use bolt_v2::bolt_v3_capital_reservation::CapitalPoolSnapshot;
use bolt_v2::bolt_v3_position_sizer::{
    FeeSlippagePolicy, PredictionMarketSizingSnapshot, ProductKind, ProductSizingSnapshot,
    SizingMode, SizingPolicy,
};
use bolt_v2::bolt_v3_position_sizer_runtime_feed::{
    PositionSizerRuntimeFeed, PositionSizerRuntimeFeedConfig, subscribe_position_sizer_runtime_feed,
};
use bolt_v2::bolt_v3_sizing_state::{
    NtDerivedSizingState, OrderLifecycleSizingSnapshot, PortfolioSizingSnapshot,
    ReservationLedgerSnapshot,
};
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3CompiledOrderKind, BoltV3CompiledOrderLiquidity, BoltV3CompiledOrderSide,
    BoltV3CompiledOrderSizingEvidence, BoltV3CompiledProductKind, BoltV3SubmitAdmissionRequest,
    BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy,
    BoltV3SubmitPositionSizerConfig, PredictionMarketOutcomeSide,
};
use nautilus_common::msgbus::{publish_order_event, switchboard};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    events::{OrderCanceled, OrderDenied, OrderEventAny, OrderExpired, OrderRejected},
    identifiers::{AccountId, ClientOrderId, InstrumentId, StrategyId, TraderId, VenueOrderId},
};
use rust_decimal::Decimal;
use ustr::Ustr;

#[test]
fn terminal_nt_order_event_releases_committed_submit_reservation() {
    let admission = Arc::new(position_sized_admission());
    arm_default(&admission);
    admission.update_position_sizing_state(fresh_sizing_state(900));
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let mut feed = PositionSizerRuntimeFeed::new(
        PositionSizerRuntimeFeedConfig {
            account_id: AccountId::from("POLYMARKET-001"),
        },
        admission.clone(),
    );
    let decision = feed
        .on_order_event(&OrderEventAny::Canceled(order_canceled_event(
            "client-order-1",
            1_100,
        )))
        .expect("terminal event for configured account should produce lifecycle decision");

    assert!(decision.accepted);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    assert_eq!(
        feed.latest_terminal_observed_at_ns(),
        Some(1_100),
        "feed should expose latest accepted terminal NT event timestamp"
    );
}

#[test]
fn subscribed_terminal_nt_order_event_releases_committed_submit_reservation() {
    let admission = Arc::new(position_sized_admission());
    arm_default(&admission);
    admission.update_position_sizing_state(fresh_sizing_state(900));
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let feed = Arc::new(Mutex::new(PositionSizerRuntimeFeed::new(
        PositionSizerRuntimeFeedConfig {
            account_id: AccountId::from("POLYMARKET-001"),
        },
        admission.clone(),
    )));
    let mut subscription = subscribe_position_sizer_runtime_feed(feed.clone());

    publish_order_event(
        switchboard::get_event_orders_topic(StrategyId::from("strategy-a")),
        &OrderEventAny::Canceled(order_canceled_event("client-order-1", 1_100)),
    );
    subscription.unsubscribe_all();

    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    assert_eq!(
        feed.lock()
            .expect("feed mutex should not be poisoned")
            .latest_terminal_observed_at_ns(),
        Some(1_100)
    );
}

#[test]
fn denied_nt_order_event_without_account_releases_matching_committed_submit_reservation() {
    let admission = Arc::new(position_sized_admission());
    arm_default(&admission);
    admission.update_position_sizing_state(fresh_sizing_state(900));
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let mut feed = PositionSizerRuntimeFeed::new(
        PositionSizerRuntimeFeedConfig {
            account_id: AccountId::from("POLYMARKET-001"),
        },
        admission.clone(),
    );
    let decision = feed
        .on_order_event(&OrderEventAny::Denied(order_denied_event(
            "client-order-1",
            1_100,
        )))
        .expect("account-less denied event should be matched by committed reservation id");

    assert!(decision.accepted);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    assert_eq!(feed.latest_terminal_observed_at_ns(), Some(1_100));
}

#[test]
fn rejected_and_expired_nt_order_events_release_matching_committed_submit_reservations() {
    assert_terminal_event_releases(
        "client-order-rejected",
        OrderEventAny::Rejected(order_rejected_event(
            "client-order-rejected",
            1_100,
            AccountId::from("POLYMARKET-001"),
        )),
    );
    assert_terminal_event_releases(
        "client-order-expired",
        OrderEventAny::Expired(order_expired_event(
            "client-order-expired",
            1_200,
            Some(AccountId::from("POLYMARKET-001")),
        )),
    );
}

#[test]
fn account_bound_terminal_nt_order_event_for_other_account_is_ignored() {
    let admission = Arc::new(position_sized_admission());
    arm_default(&admission);
    admission.update_position_sizing_state(fresh_sizing_state(900));
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let mut feed = PositionSizerRuntimeFeed::new(
        PositionSizerRuntimeFeedConfig {
            account_id: AccountId::from("POLYMARKET-001"),
        },
        admission.clone(),
    );

    assert!(
        feed.on_order_event(&OrderEventAny::Rejected(order_rejected_event(
            "client-order-1",
            1_100,
            AccountId::from("OTHER-ACCOUNT"),
        )))
        .is_none()
    );
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    assert_eq!(feed.latest_terminal_observed_at_ns(), None);
}

fn assert_terminal_event_releases(client_order_id: &str, event: OrderEventAny) {
    let admission = Arc::new(position_sized_admission());
    arm_default(&admission);
    admission.update_position_sizing_state(fresh_sizing_state(900));
    admission
        .admit_at(&sized_submit_request(client_order_id), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let observed_at_ns = event.ts_event().as_u64();
    let mut feed = PositionSizerRuntimeFeed::new(
        PositionSizerRuntimeFeedConfig {
            account_id: AccountId::from("POLYMARKET-001"),
        },
        admission.clone(),
    );
    let decision = feed
        .on_order_event(&event)
        .expect("terminal event should release matching committed reservation");

    assert!(decision.accepted);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    assert_eq!(feed.latest_terminal_observed_at_ns(), Some(observed_at_ns));
}

fn position_sized_admission() -> BoltV3SubmitAdmissionState {
    BoltV3SubmitAdmissionState::new_unarmed_with_position_sizer(
        Arc::new(support::RecordingDecisionEvidenceWriter::default()),
        BoltV3SubmitPositionSizerConfig {
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            collateral_currency: "PUSD".to_string(),
            capital_pool: CapitalPoolSnapshot {
                source: "bolt_submit_sizer_bootstrap".to_string(),
                observed_at_ns: 900,
                pool_id: "pool-1".to_string(),
                max_pool_liability: Decimal::new(10, 0),
                committed_liability: Decimal::ZERO,
                max_snapshot_age_ns: 500,
            },
            policy: SizingPolicy {
                mode: SizingMode::RejectOnly,
                max_order_liability: Some(Decimal::new(10, 0)),
                min_remaining_pool_balance: None,
                fee_slippage_policy: Some(FeeSlippagePolicy {
                    max_fee_liability: Decimal::new(10, 2),
                    max_slippage_liability: Decimal::new(20, 2),
                }),
            },
        },
    )
}

fn arm_default(admission: &BoltV3SubmitAdmissionState) {
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            10,
            Decimal::new(10, 0),
        ))
        .expect("valid gate report should arm admission");
}

fn sized_submit_request(client_order_id: &str) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        client_order_id: client_order_id.to_string(),
        instrument_id: "condition-yes.POLYMARKET".to_string(),
        notional: Decimal::new(4, 0),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        position_sizing: Some(BoltV3CompiledOrderSizingEvidence {
            venue_id: "POLYMARKET".to_string(),
            product_kind: BoltV3CompiledProductKind::PredictionMarketBinary,
            side: BoltV3CompiledOrderSide::Buy,
            quantity: Decimal::new(10, 0),
            effective_price: Decimal::new(40, 2),
            order_kind: BoltV3CompiledOrderKind::Limit,
            liquidity: BoltV3CompiledOrderLiquidity::Taker,
            quote_set_id: None,
            prediction_market_outcome: Some(PredictionMarketOutcomeSide::Yes),
        }),
    }
}

fn fresh_sizing_state(observed_at_ns: u64) -> NtDerivedSizingState {
    NtDerivedSizingState {
        source: "nt_sizing_state".to_string(),
        observed_at_ns,
        portfolio: PortfolioSizingSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns,
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            collateral_currency: "PUSD".to_string(),
            free_collateral: Decimal::new(100, 0),
            total_equity: Decimal::new(100, 0),
        },
        order_lifecycle: OrderLifecycleSizingSnapshot {
            source: "nt_open_order_cache".to_string(),
            observed_at_ns,
            open_order_count: 0,
            all_open_orders_attributed: true,
        },
        product_state: ProductSizingSnapshot::PredictionMarketBinary(
            PredictionMarketSizingSnapshot {
                source: "nt_prediction_market_snapshot".to_string(),
                observed_at_ns,
                yes_instrument_id: "condition-yes.POLYMARKET".to_string(),
                no_instrument_id: "condition-no.POLYMARKET".to_string(),
                yes_position: Decimal::new(10, 0),
                no_position: Decimal::ZERO,
                pusd_allowance: Decimal::new(100, 0),
                conditional_token_allowance: Decimal::new(10, 0),
                collateral_coupled_group_id: "group-1".to_string(),
            },
        ),
        reservation_snapshot: ReservationLedgerSnapshot {
            source: "bolt_reservation_ledger".to_string(),
            observed_at_ns,
            all_live_reservations_attributed: true,
        },
        loss_snapshot: None,
    }
}

fn order_canceled_event(client_order_id: &str, ts_event: u64) -> OrderCanceled {
    OrderCanceled::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from("condition-yes.POLYMARKET"),
        ClientOrderId::from(client_order_id),
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
        Some(VenueOrderId::from("venue-order-1")),
        Some(AccountId::from("POLYMARKET-001")),
    )
}

fn order_denied_event(client_order_id: &str, ts_event: u64) -> OrderDenied {
    OrderDenied::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from("condition-yes.POLYMARKET"),
        ClientOrderId::from(client_order_id),
        Ustr::from("test-denied"),
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
    )
}

fn order_rejected_event(
    client_order_id: &str,
    ts_event: u64,
    account_id: AccountId,
) -> OrderRejected {
    OrderRejected::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from("condition-yes.POLYMARKET"),
        ClientOrderId::from(client_order_id),
        account_id,
        Ustr::from("test-rejected"),
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
        false,
    )
}

fn order_expired_event(
    client_order_id: &str,
    ts_event: u64,
    account_id: Option<AccountId>,
) -> OrderExpired {
    OrderExpired::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from("condition-yes.POLYMARKET"),
        ClientOrderId::from(client_order_id),
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
        Some(VenueOrderId::from("venue-order-1")),
        account_id,
    )
}
