use crate::support;

use std::{
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};

use bolt_v2::bolt_v3_capital_admission::{
    CapitalAdmissionLifecycleAction, CapitalAdmissionPolicy, PredictionMarketAdmissionSnapshot,
    ProductAdmissionSnapshot, ProductKind,
};
use bolt_v2::bolt_v3_capital_admission_runtime_feed::{
    CapitalAdmissionRuntimeFeed, CapitalAdmissionRuntimeFeedConfig,
    POLYMARKET_VENUE_TRUTH_REST_SOURCE, subscribe_capital_admission_runtime_feed,
};
use bolt_v2::bolt_v3_capital_admission_state::{
    NtDerivedCapitalAdmissionState, OrderLifecycleCapitalAdmissionSnapshot,
    PortfolioCapitalAdmissionSnapshot, ReservationLedgerSnapshot, VenueSpendabilitySnapshot,
};
use bolt_v2::bolt_v3_capital_reservation::CapitalPoolSnapshot;
use bolt_v2::bolt_v3_kill_switch::{KillSwitchState, KillSwitchStateKind};
use bolt_v2::bolt_v3_providers::polymarket::{
    PolymarketVenueTruthInput, build_polymarket_venue_truth_snapshot,
};
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3CapitalAdmissionRejectReason, BoltV3CompiledOrderAdmissionEvidence,
    BoltV3CompiledOrderKind, BoltV3CompiledOrderLiquidity, BoltV3CompiledOrderSide,
    BoltV3CompiledProductKind, BoltV3KillSwitchForcedReductionClaim,
    BoltV3KillSwitchForcedReductionPolicy, BoltV3RiskReducingExitProof, BoltV3SubmitAdmissionError,
    BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitCapitalAdmissionConfig,
    BoltV3SubmitCapitalAdmissionNtComponents, BoltV3SubmitCapitalAdmissionOpenOrderEvidence,
    BoltV3SubmitCapitalAdmissionOpenOrderReservation, BoltV3SubmitIntentKind,
    BoltV3SubmitLifecyclePolicy, PredictionMarketOutcomeSide,
};
use bolt_v2::bolt_v3_venue_truth::{
    VenueTruthCaptureFailureEvidence, VenueTruthSettlementExplanation, VenueTruthSnapshot,
};
use nautilus_common::msgbus::{
    TypedHandler, publish_account_state, publish_order_event, publish_portfolio_snapshot,
    publish_position_event, subscribe_account_state, subscribe_order_events,
    subscribe_portfolio_snapshot, subscribe_position_events, switchboard,
    unsubscribe_account_state, unsubscribe_order_events, unsubscribe_portfolio_snapshot,
    unsubscribe_position_events,
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    enums::{
        AccountType, CurrencyType, LiquiditySide, OrderSide, OrderType, PositionAdjustmentType,
        PositionSide,
    },
    events::{
        AccountState, OrderAccepted, OrderCanceled, OrderDenied, OrderEventAny, OrderExpired,
        OrderFilled, OrderRejected, OrderSubmitted, PortfolioSnapshot, PositionAdjusted,
        PositionEvent,
    },
    identifiers::{
        AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId, TradeId, TraderId,
        VenueOrderId,
    },
    types::{AccountBalance, Currency, Money, Price, Quantity},
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
fn runtime_feed_uses_verified_nt_msgbus_symbols() {
    let _ = subscribe_account_state;
    let _ = subscribe_order_events;
    let _ = subscribe_portfolio_snapshot;
    let _ = subscribe_position_events;
    let _ = unsubscribe_account_state;
    let _ = unsubscribe_order_events;
    let _ = unsubscribe_portfolio_snapshot;
    let _ = unsubscribe_position_events;
    let _ = std::any::type_name::<TypedHandler<AccountState>>();
    let _ = std::any::type_name::<TypedHandler<OrderEventAny>>();
    let _ = std::any::type_name::<TypedHandler<PortfolioSnapshot>>();
    let _ = std::any::type_name::<TypedHandler<PositionEvent>>();

    let source = support::repo_text("src/bolt_v3_capital_admission_runtime_feed.rs");
    for needle in [
        "subscribe_account_state",
        "subscribe_order_events",
        "subscribe_portfolio_snapshot",
        "subscribe_position_events",
        "unsubscribe_account_state",
        "unsubscribe_order_events",
        "unsubscribe_portfolio_snapshot",
        "unsubscribe_position_events",
    ] {
        assert!(source.contains(needle), "runtime feed must use `{needle}`");
    }
}

#[test]
#[should_panic(expected = "capital admission runtime order-event feed lock poisoned")]
fn subscribed_order_event_panics_on_poisoned_capital_admission_feed_lock() {
    let feed = poisoned_capital_admission_runtime_feed();
    let _subscription = subscribe_capital_admission_runtime_feed(feed);

    publish_order_event(
        switchboard::get_event_order_topic(StrategyId::from("strategy-a")),
        &OrderEventAny::Canceled(order_canceled_event("client-order-1", 1_100)),
    );
}

#[test]
#[should_panic(expected = "capital admission runtime position-event feed lock poisoned")]
fn subscribed_position_event_panics_on_poisoned_capital_admission_feed_lock() {
    let feed = poisoned_capital_admission_runtime_feed();
    let _subscription = subscribe_capital_admission_runtime_feed(feed);

    publish_position_event(
        "events.position.ACCOUNT-001".into(),
        &adjusted_position_event(AccountId::from("ACCOUNT-001"), 1_100),
    );
}

#[test]
#[should_panic(expected = "capital admission runtime account-state feed lock poisoned")]
fn subscribed_account_state_panics_on_poisoned_capital_admission_feed_lock() {
    let feed = poisoned_capital_admission_runtime_feed();
    let _subscription = subscribe_capital_admission_runtime_feed(feed);

    publish_account_state(
        "events.account.ACCOUNT-001".into(),
        &account_state(AccountId::from("ACCOUNT-001"), "USD", 1_100, 45.0),
    );
}

#[test]
#[should_panic(expected = "capital admission runtime portfolio-snapshot feed lock poisoned")]
fn subscribed_portfolio_snapshot_panics_on_poisoned_capital_admission_feed_lock() {
    let feed = poisoned_capital_admission_runtime_feed();
    let _subscription = subscribe_capital_admission_runtime_feed(feed);

    publish_portfolio_snapshot(
        "events.portfolio.ACCOUNT-001".into(),
        &portfolio_snapshot(AccountId::from("ACCOUNT-001"), "USD", 1_100, 45.0),
    );
}

#[test]
fn subscribed_account_and_portfolio_events_remain_advisory_without_venue_truth() {
    let admission = Arc::new(capital_admission_configured_admission());
    let feed = Arc::new(Mutex::new(CapitalAdmissionRuntimeFeed::new(
        runtime_feed_config(),
        admission.clone(),
    )));
    let mut subscription = subscribe_capital_admission_runtime_feed(feed);

    publish_account_state(
        "events.account.ACCOUNT-001".into(),
        &account_state(AccountId::from("ACCOUNT-001"), "USD", 1_000, 45.0),
    );
    publish_portfolio_snapshot(
        "events.portfolio.ACCOUNT-001".into(),
        &portfolio_snapshot(AccountId::from("ACCOUNT-001"), "USD", 1_100, 50.0),
    );
    subscription.unsubscribe_all();

    assert_eq!(
        admission.capital_admission_state_snapshot(),
        None,
        "NT account and portfolio events are advisory and must not satisfy Polymarket money readiness"
    );
}

#[test]
fn polymarket_venue_truth_snapshot_alone_promotes_money_readiness() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    let components = feed
        .on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
            1_200,
            Decimal::new(45_000_000, 0),
            Decimal::new(40_000_000, 0),
        ))
        .expect("initial venue truth baseline should be explainable")
        .expect("accepted venue truth alone should publish money components");

    assert_eq!(
        components.venue_spendability.source,
        "polymarket_venue_truth_rest"
    );
    assert_eq!(
        components.venue_spendability.spendable_collateral,
        Decimal::new(45, 0)
    );
    assert_eq!(
        components.venue_spendability.collateral_allowance,
        Decimal::new(40, 0)
    );
    assert_eq!(components.portfolio.source, "polymarket_venue_truth_rest");
    assert_eq!(components.portfolio.free_collateral, Decimal::new(45, 0));
    assert_eq!(components.portfolio.total_equity, Decimal::new(45, 0));
    assert_eq!(
        admission
            .capital_admission_state_snapshot()
            .expect("promoted components should update admission")
            .venue_spendability
            .source,
        "polymarket_venue_truth_rest"
    );
}

#[test]
fn polymarket_venue_truth_snapshot_promotes_open_orders_and_positions() {
    let admission = Arc::new(polymarket_capital_admission_configured_admission());
    let mut feed =
        CapitalAdmissionRuntimeFeed::new(polymarket_runtime_feed_config(), admission.clone());

    let components = feed
        .on_venue_truth_snapshot(polymarket_venue_truth_snapshot_with_orders_and_positions(
            1_200,
            Decimal::new(45_000_000, 0),
            Decimal::new(40_000_000, 0),
        ))
        .expect("initial venue truth baseline should be explainable")
        .expect("accepted venue truth alone should publish full money components");

    assert_eq!(
        components.order_lifecycle.source,
        POLYMARKET_VENUE_TRUTH_REST_SOURCE
    );
    assert_eq!(components.order_lifecycle.open_order_count, 1);
    assert!(components.order_lifecycle.all_open_orders_attributed);

    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(product.source, POLYMARKET_VENUE_TRUTH_REST_SOURCE);
    assert_eq!(product.observed_at_ns, 1_200);
    assert_eq!(product.yes_position, Decimal::new(7, 0));
    assert_eq!(product.no_position, Decimal::new(2, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::new(9, 0));
    assert_eq!(product.collateral_allowance, Decimal::new(40, 0));
}

#[test]
fn venue_truth_settlement_recorded_through_feed_explains_next_capture() {
    let admission = Arc::new(polymarket_capital_admission_configured_admission());
    let mut feed =
        CapitalAdmissionRuntimeFeed::new(polymarket_runtime_feed_config(), admission.clone());

    feed.on_venue_truth_snapshot(polymarket_venue_truth_snapshot_with_position(
        1_000,
        Decimal::new(50_000_000, 0),
        Decimal::new(40_000_000, 0),
        "yes123",
        4.0,
    ))
    .expect("baseline venue truth should reconcile");
    feed.record_venue_truth_settlement(VenueTruthSettlementExplanation {
        settlement_key: "yes123:P-FEED-SETTLEMENT".to_string(),
        market_id: "condition".to_string(),
        product_id: "yes123".to_string(),
        side: OrderSide::Sell,
        settled_quantity: Decimal::new(4, 0),
        payout_per_share: Decimal::ONE,
        collateral_payout: Decimal::new(4, 0),
    })
    .expect("settlement should record through the production feed");

    feed.on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
        1_100,
        Decimal::new(54_000_000, 0),
        Decimal::new(40_000_000, 0),
    ))
    .expect("booked settlement should explain the next venue capture");

    assert!(
        admission.capital_admission_state_snapshot().is_some(),
        "accepted settlement capture should keep capital admission published"
    );
}

#[test]
fn accepted_venue_truth_open_orders_override_stale_nt_live_order_attribution() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    let _ = feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_025,
        AccountId::from("ACCOUNT-001"),
    )));
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_050)
        .expect("test reservation should be admitted after rebuilding the startup gate")
        .commit_submitted();

    let components = feed
        .on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
            1_200,
            Decimal::new(100_000_000, 0),
            Decimal::new(100_000_000, 0),
        ))
        .expect("initial venue truth baseline should be explainable")
        .expect("accepted venue truth should publish components");

    assert_eq!(
        components.order_lifecycle.source,
        POLYMARKET_VENUE_TRUTH_REST_SOURCE
    );
    assert_eq!(
        components.order_lifecycle.open_order_count, 0,
        "accepted venue truth open-order count must not be overwritten by stale NT attribution memory"
    );
    assert!(components.order_lifecycle.all_open_orders_attributed);
}

#[test]
fn accepted_venue_truth_survives_later_nt_cache_seed_and_reservation_rebuild() {
    let admission = Arc::new(polymarket_capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed =
        CapitalAdmissionRuntimeFeed::new(polymarket_runtime_feed_config(), admission.clone());

    let components = feed
        .on_venue_truth_snapshot(polymarket_venue_truth_snapshot_with_orders_and_positions(
            1_200,
            Decimal::new(100_000_000, 0),
            Decimal::new(100_000_000, 0),
        ))
        .expect("initial venue truth baseline should be explainable")
        .expect("accepted venue truth should publish components");
    assert_eq!(
        components.order_lifecycle.source,
        POLYMARKET_VENUE_TRUTH_REST_SOURCE
    );

    let seeded_components = feed
        .seed_cache_snapshot(
            vec!["stale-nt-cache-order".to_string()],
            Decimal::new(99, 0),
            Decimal::new(88, 0),
            1_300,
        )
        .expect("accepted venue truth remains sufficient after advisory NT cache seed");
    assert_eq!(
        seeded_components.order_lifecycle.source,
        POLYMARKET_VENUE_TRUTH_REST_SOURCE
    );
    assert_eq!(seeded_components.order_lifecycle.open_order_count, 1);
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = seeded_components.product_state;
    assert_eq!(product.source, POLYMARKET_VENUE_TRUTH_REST_SOURCE);
    assert_eq!(product.yes_position, Decimal::new(7, 0));
    assert_eq!(product.no_position, Decimal::new(2, 0));

    let mut recovered_reservation = open_order_reservation(
        "client-order-1",
        "client-order-1#rebuilt",
        Decimal::new(43, 1),
    );
    recovered_reservation.observed_at_ns = 1_350;
    let rebuild = admission
        .rebuild_capital_admission_open_order_reservations(vec![recovered_reservation], 1_350);
    assert!(rebuild.accepted, "rebuild should accept: {rebuild:?}");
    let state = admission
        .capital_admission_state_snapshot()
        .expect("accepted venue truth should remain capital admission state");
    assert_eq!(
        state.order_lifecycle.source,
        POLYMARKET_VENUE_TRUTH_REST_SOURCE
    );
    assert_eq!(state.order_lifecycle.open_order_count, 1);
    assert!(state.order_lifecycle.all_open_orders_attributed);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}

#[test]
fn accepted_venue_truth_portfolio_survives_later_nt_portfolio_and_account_seed() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    let components = feed
        .on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
            1_200,
            Decimal::new(45_000_000, 0),
            Decimal::new(40_000_000, 0),
        ))
        .expect("initial venue truth baseline should be explainable")
        .expect("accepted venue truth should publish components");
    assert_eq!(
        components.portfolio.source,
        POLYMARKET_VENUE_TRUTH_REST_SOURCE
    );
    assert_eq!(components.portfolio.free_collateral, Decimal::new(45, 0));

    let portfolio_components = feed
        .on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_300,
            99.0,
        ))
        .expect("advisory NT portfolio should republish accepted venue truth");
    assert_eq!(
        portfolio_components.portfolio.source,
        POLYMARKET_VENUE_TRUTH_REST_SOURCE
    );
    assert_eq!(
        portfolio_components.portfolio.free_collateral,
        Decimal::new(45, 0)
    );

    let seeded_components = feed
        .seed_account_portfolio_snapshot(Decimal::new(88, 0), Decimal::new(88, 0), 1_400)
        .expect("advisory account seed should republish accepted venue truth");
    assert_eq!(
        seeded_components.portfolio.source,
        POLYMARKET_VENUE_TRUTH_REST_SOURCE
    );
    assert_eq!(
        seeded_components.portfolio.free_collateral,
        Decimal::new(45, 0)
    );

    let state = admission
        .capital_admission_state_snapshot()
        .expect("accepted venue truth should remain published");
    assert_eq!(state.portfolio.source, POLYMARKET_VENUE_TRUTH_REST_SOURCE);
    assert_eq!(state.portfolio.free_collateral, Decimal::new(45, 0));
}

#[test]
fn polymarket_venue_truth_allowance_is_not_min_clamped_by_nt_account_free_collateral() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission);

    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        1_000,
        10.0,
    ));
    let _ = feed.on_portfolio_snapshot(&portfolio_snapshot(
        AccountId::from("ACCOUNT-001"),
        "USD",
        1_050,
        10.0,
    ));

    let components = feed
        .on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
            1_200,
            Decimal::new(45_000_000, 0),
            Decimal::new(40_000_000, 0),
        ))
        .expect("initial venue truth baseline should be explainable")
        .expect("accepted venue truth should publish components");

    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(
        product.collateral_allowance,
        Decimal::new(40, 0),
        "accepted venue truth allowance must not be min-clamped by stale NT free collateral"
    );
}

#[test]
fn venue_truth_capture_failure_suspends_all_admission_and_success_auto_resumes() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let admission = Arc::new(capital_admission_configured_admission_with_writer(
        writer.clone(),
    ));
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    let _ = feed.on_portfolio_snapshot(&portfolio_snapshot(
        AccountId::from("ACCOUNT-001"),
        "USD",
        950,
        100.0,
    ));
    feed.on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
        1_000,
        Decimal::new(100_000_000, 0),
        Decimal::new(100_000_000, 0),
    ))
    .expect("initial venue-truth baseline should reconcile")
    .expect("account, portfolio, and venue truth should publish components");
    feed.seed_open_order_cache(Vec::<String>::new(), 1_025)
        .expect("empty startup cache should publish attributed order lifecycle");
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_050)
        .expect("fresh sizing state should admit before degraded venue authority")
        .commit_submitted();

    admission.suspend_capital_admission_for_venue_truth_capture_failure(
        VenueTruthCaptureFailureEvidence {
            source: POLYMARKET_VENUE_TRUTH_REST_SOURCE.to_string(),
            observed_at_ns: 1_100,
            endpoint: "clob_balance_allowance".to_string(),
            error_class: "transport".to_string(),
            captures_missed: 1,
        },
    );

    assert_eq!(
        admission.kill_switch_state_kind(),
        KillSwitchStateKind::Armed
    );
    assert_eq!(admission.capital_admission_reconciled(), Some(false));
    let capture_failures = writer.venue_truth_capture_failures();
    assert_eq!(capture_failures.len(), 1);
    assert_eq!(capture_failures[0].endpoint, "clob_balance_allowance");
    assert_eq!(capture_failures[0].error_class, "transport");
    assert_eq!(capture_failures[0].captures_missed, 1);
    assert!(
        matches!(
            admission.admit_at(&risk_reducing_exit_submit_request("client-order-2"), 1_101),
            Err(BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
                reason: BoltV3CapitalAdmissionRejectReason::ReconciliationRequired
            })
        ),
        "degraded venue authority must suspend risk-reducing exits too"
    );

    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        1_150,
        100.0,
    ));
    assert_eq!(
        admission.capital_admission_reconciled(),
        Some(false),
        "NT-driven publish from the long-lived feed must not clear capture-failure suspension"
    );
    feed.on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
        1_200,
        Decimal::new(100_000_000, 0),
        Decimal::new(100_000_000, 0),
    ))
    .expect("successful venue-truth capture should reconcile")
    .expect("accepted venue truth should publish components");

    assert_eq!(admission.capital_admission_reconciled(), Some(true));
}

#[test]
fn accepted_capture_at_failure_watermark_does_not_clear_capture_failure_suspension() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    feed.on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
        1_000,
        Decimal::new(100_000_000, 0),
        Decimal::new(100_000_000, 0),
    ))
    .expect("initial venue-truth baseline should reconcile")
    .expect("accepted venue truth should publish components");

    admission.suspend_capital_admission_for_venue_truth_capture_failure(
        VenueTruthCaptureFailureEvidence {
            source: POLYMARKET_VENUE_TRUTH_REST_SOURCE.to_string(),
            observed_at_ns: 1_100,
            endpoint: "clob_balance_allowance".to_string(),
            error_class: "transport".to_string(),
            captures_missed: 1,
        },
    );
    assert_eq!(admission.capital_admission_reconciled(), Some(false));

    feed.on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
        1_100,
        Decimal::new(100_000_000, 0),
        Decimal::new(100_000_000, 0),
    ))
    .expect("equal-watermark capture should reconcile")
    .expect("accepted venue truth should publish components");
    assert_eq!(
        admission.capital_admission_reconciled(),
        Some(false),
        "accepted_capture == failure_observed must not clear degraded-authority suspension"
    );

    feed.on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
        1_101,
        Decimal::new(100_000_000, 0),
        Decimal::new(100_000_000, 0),
    ))
    .expect("strictly later capture should reconcile")
    .expect("accepted venue truth should publish components");
    assert_eq!(admission.capital_admission_reconciled(), Some(true));
}

#[test]
fn capital_admission_runtime_subscription_drop_unsubscribes_all_handlers() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let feed = Arc::new(Mutex::new(CapitalAdmissionRuntimeFeed::new(
        runtime_feed_config(),
        admission.clone(),
    )));
    let mut subscription = subscribe_capital_admission_runtime_feed(feed.clone());
    subscription.unsubscribe_all();

    publish_account_state(
        "events.account.ACCOUNT-001".into(),
        &account_state(AccountId::from("ACCOUNT-001"), "USD", 2_000, 80.0),
    );
    publish_portfolio_snapshot(
        "events.portfolio.ACCOUNT-001".into(),
        &portfolio_snapshot(AccountId::from("ACCOUNT-001"), "USD", 2_100, 90.0),
    );
    publish_position_event(
        "events.position.ACCOUNT-001".into(),
        &adjusted_position_event(AccountId::from("ACCOUNT-001"), 2_200),
    );
    publish_order_event(
        switchboard::get_event_order_topic(StrategyId::from("strategy-a")),
        &OrderEventAny::Canceled(order_canceled_event("client-order-1", 2_300)),
    );

    assert_eq!(
        admission.capital_admission_state_observed_at_ns(),
        Some(1_000)
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    assert_eq!(
        feed.lock()
            .expect("feed mutex should not be poisoned")
            .latest_terminal_observed_at_ns(),
        None
    );
}

#[test]
fn feed_waits_for_matching_account_identity_before_publish() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.on_venue_truth_snapshot(polymarket_venue_truth_snapshot(
            1_000,
            Decimal::new(45_000_000, 0),
            Decimal::new(40_000_000, 0),
        ))
        .expect("matching venue truth snapshot should reconcile")
        .is_some()
    );
    assert!(admission.capital_admission_state_snapshot().is_some());

    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let mut wrong_account_venue_truth = polymarket_venue_truth_snapshot(
        1_025,
        Decimal::new(45_000_000, 0),
        Decimal::new(40_000_000, 0),
    );
    wrong_account_venue_truth.account_id = AccountId::from("OTHER-ACCOUNT");
    assert!(
        feed.on_venue_truth_snapshot(wrong_account_venue_truth)
            .expect("wrong-account initial venue truth should not be a reconciliation divergence")
            .is_none()
    );
    assert!(
        feed.on_venue_spendability_snapshot(venue_spendability_snapshot(1_050, 45, 40))
            .is_none(),
        "wrong-account venue truth must not seed portfolio/product state for a later matching spendability snapshot"
    );
    assert_eq!(admission.capital_admission_state_snapshot(), None);

    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            50.0
        ))
        .is_none()
    );
    assert_eq!(admission.capital_admission_state_snapshot(), None);

    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("OTHER-ACCOUNT"),
            "USD",
            1_200,
            45.0
        ))
        .is_none()
    );
    assert_eq!(admission.capital_admission_state_snapshot(), None);

    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_300,
            45.0
        ))
        .is_none()
    );
    assert_eq!(admission.capital_admission_state_snapshot(), None);

    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_400,
            50.0
        ))
        .is_none()
    );
    assert_eq!(admission.capital_admission_state_snapshot(), None);
}

#[test]
fn feed_does_not_derive_default_venue_spendability_from_nt_account_free_collateral() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_000,
            45.0,
        ))
        .is_none(),
        "NT AccountState is advisory-only and must not create venue spendability"
    );
    assert_eq!(admission.capital_admission_state_snapshot(), None);
}

#[test]
fn feed_derives_collateral_allowance_from_venue_allowance() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.on_venue_spendability_snapshot(venue_spendability_snapshot(950, 30, 25))
            .is_none()
    );
    let components = feed
        .on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_000,
            100.0,
        ))
        .expect("account and spendability should publish components");

    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(product.collateral_allowance, Decimal::new(25, 0));

    let state = admission
        .capital_admission_state_snapshot()
        .expect("published components should update admission sizing state");
    assert_eq!(
        state.venue_spendability.spendable_collateral,
        Decimal::new(30, 0)
    );
    assert_eq!(
        state.venue_spendability.collateral_allowance,
        Decimal::new(25, 0)
    );
}

#[test]
fn account_update_does_not_make_external_venue_spendability_fresh() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission);

    assert!(
        feed.on_venue_spendability_snapshot(venue_spendability_snapshot(100, 30, 25))
            .is_none()
    );
    let components = feed
        .on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            10_000,
            100.0,
        ))
        .expect("account and venue state should publish");

    assert_eq!(
        components.venue_spendability.observed_at_ns, 100,
        "fresh NT account state must not refresh externally sourced venue evidence"
    );
}

#[test]
fn recomputed_product_allowance_carries_fresh_component_timestamp() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut config = runtime_feed_config();
    config.startup_observed_at_ns = 0;
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = &mut config.product_state;
    product.observed_at_ns = 0;
    let mut feed = CapitalAdmissionRuntimeFeed::new(config, admission);

    assert!(
        feed.on_venue_spendability_snapshot(venue_spendability_snapshot(10_000, 30, 25))
            .is_none()
    );
    let components = feed
        .on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            10_000,
            100.0,
        ))
        .expect("complete fresh account components should publish");

    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(product.collateral_allowance, Decimal::new(25, 0));
    assert_eq!(
        product.observed_at_ns, 10_000,
        "recomputed product allowance must be timestamped with the fresh constraining inputs"
    );
}

#[test]
fn feed_does_not_use_spendable_collateral_as_product_allowance() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.on_venue_spendability_snapshot(venue_spendability_snapshot(950, 25, 30))
            .is_none()
    );
    let components = feed
        .on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_000,
            100.0,
        ))
        .expect("complete spendability/account state should publish");

    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(
        product.collateral_allowance,
        Decimal::new(30, 0),
        "product allowance comes from venue allowance and is not min-clamped by spendable collateral"
    );
}

#[test]
fn feed_ignores_spendability_identity_mismatch_until_matching_snapshot_arrives() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.on_venue_spendability_snapshot(venue_spendability_snapshot(900, 100, 100))
            .is_none()
    );
    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0
        ))
        .is_some()
    );

    let mut mismatched = venue_spendability_snapshot(1_100, 100, 100);
    mismatched.venue_id = "VENUE-B".to_string();
    let components = feed
        .on_venue_spendability_snapshot(mismatched)
        .expect("mismatched spendability must not clear the last valid state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(product.collateral_allowance, Decimal::new(100, 0));
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_200,
            100.0
        ))
        .is_some()
    );

    assert!(
        feed.on_venue_spendability_snapshot(venue_spendability_snapshot(1_300, 50, 50))
            .is_some()
    );
}

#[test]
fn feed_ignores_older_spendability_snapshot_after_newer_one() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.on_venue_spendability_snapshot(venue_spendability_snapshot(1_000, 100, 40))
            .is_none()
    );
    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_050,
            100.0
        ))
        .is_some()
    );

    let components = feed
        .on_venue_spendability_snapshot(venue_spendability_snapshot(900, 100, 5))
        .expect("older spendability should not clear or regress the latest snapshot");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(product.collateral_allowance, Decimal::new(40, 0));
}

#[test]
fn feed_ignores_account_state_for_other_collateral_currency() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_000,
            50.0
        ))
        .is_none()
    );
    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "EUR",
            1_100,
            45.0
        ))
        .is_none()
    );
    assert_eq!(admission.capital_admission_state_snapshot(), None);
}

#[test]
fn capital_admission_cache_seed_updates_open_order_lifecycle_and_rebuilds_empty() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        1_000,
        45.0,
    ));
    seed_venue_spendability(&mut feed, 1_050);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            50.0
        ))
        .is_some()
    );
    assert!(
        feed.seed_open_order_cache(Vec::<String>::new(), 1_200)
            .is_some()
    );

    let rebuild = admission.rebuild_capital_admission_open_order_reservations(Vec::new(), 1_200);

    assert!(rebuild.accepted);
    assert_eq!(admission.capital_admission_reconciled(), Some(true));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("cache seed should publish sizing components");
    assert_eq!(state.order_lifecycle.source, "nt_open_order_cache");
    assert_eq!(state.order_lifecycle.observed_at_ns, 1_200);
    assert_eq!(state.order_lifecycle.open_order_count, 0);
    assert!(state.order_lifecycle.all_open_orders_attributed);
}

#[test]
fn account_state_after_empty_startup_cache_reconciles_empty_gate_without_portfolio_snapshot() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    seed_venue_spendability(&mut feed, 950);
    assert!(
        feed.seed_open_order_cache(Vec::<String>::new(), 1_000)
            .is_none()
    );
    let rebuild = admission.rebuild_capital_admission_open_order_reservations(Vec::new(), 1_000);
    assert!(!rebuild.accepted);
    assert_eq!(admission.capital_admission_reconciled(), Some(false));

    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            100.0,
        ))
        .is_some()
    );

    assert_eq!(admission.capital_admission_reconciled(), Some(true));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("account state should publish sizing state");
    assert_eq!(state.portfolio.source, "nt_account_state");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
    assert!(state.order_lifecycle.all_open_orders_attributed);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_150)
        .expect("empty startup cache plus account state should admit")
        .commit_submitted();
}

#[test]
fn account_state_after_unattributed_live_order_keeps_gate_unreconciled() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    seed_venue_spendability(&mut feed, 950);
    let _ = feed.on_order_event(&OrderEventAny::Submitted(order_submitted_event(
        "external-client-order",
        1_000,
        AccountId::from("ACCOUNT-001"),
    )));

    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            100.0,
        ))
        .is_some()
    );

    assert_eq!(admission.capital_admission_reconciled(), Some(false));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("account state should publish unreconciled sizing state");
    assert_eq!(state.order_lifecycle.open_order_count, 1);
    assert!(!state.order_lifecycle.all_open_orders_attributed);
    assert_eq!(
        admission
            .admit_at(&capital_admission_submit_request("client-order-1"), 1_150)
            .expect_err("unattributed live order must keep submit admission closed"),
        BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
            reason: BoltV3CapitalAdmissionRejectReason::ReconciliationRequired,
        }
    );
}

#[test]
fn unattributed_live_order_after_empty_reconcile_recloses_gate() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    seed_venue_spendability(&mut feed, 950);
    assert!(
        feed.seed_open_order_cache(Vec::<String>::new(), 1_000)
            .is_none()
    );
    let rebuild = admission.rebuild_capital_admission_open_order_reservations(Vec::new(), 1_000);
    assert!(!rebuild.accepted);
    assert_eq!(admission.capital_admission_reconciled(), Some(false));

    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            100.0,
        ))
        .is_some()
    );
    assert_eq!(admission.capital_admission_reconciled(), Some(true));

    let _ = feed.on_order_event(&OrderEventAny::Submitted(order_submitted_event(
        "external-client-order",
        1_200,
        AccountId::from("ACCOUNT-001"),
    )));

    assert_eq!(admission.capital_admission_reconciled(), Some(false));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("unattributed live order should publish unreconciled sizing state");
    assert_eq!(state.order_lifecycle.open_order_count, 1);
    assert!(!state.order_lifecycle.all_open_orders_attributed);
    assert_eq!(
        admission
            .admit_at(&capital_admission_submit_request("client-order-1"), 1_250)
            .expect_err("unattributed live order must re-close submit admission"),
        BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
            reason: BoltV3CapitalAdmissionRejectReason::ReconciliationRequired,
        }
    );
}

#[test]
fn terminal_event_after_unattributed_live_order_reopens_empty_gate() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    seed_venue_spendability(&mut feed, 950);
    assert!(
        feed.seed_open_order_cache(Vec::<String>::new(), 1_000)
            .is_none()
    );
    let rebuild = admission.rebuild_capital_admission_open_order_reservations(Vec::new(), 1_000);
    assert!(!rebuild.accepted);
    assert_eq!(admission.capital_admission_reconciled(), Some(false));

    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            100.0,
        ))
        .is_some()
    );
    assert_eq!(admission.capital_admission_reconciled(), Some(true));

    let _ = feed.on_order_event(&OrderEventAny::Submitted(order_submitted_event(
        "external-client-order",
        1_200,
        AccountId::from("ACCOUNT-001"),
    )));
    assert_eq!(admission.capital_admission_reconciled(), Some(false));

    let _ = feed.on_order_event(&OrderEventAny::Canceled(order_canceled_event(
        "external-client-order",
        1_300,
    )));

    assert_eq!(admission.capital_admission_reconciled(), Some(true));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("terminal event should publish reconciled empty lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
    assert!(state.order_lifecycle.all_open_orders_attributed);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_350)
        .expect("empty terminal lifecycle should reopen submit admission")
        .commit_submitted();
}

#[test]
fn capital_admission_cache_seed_updates_configured_yes_no_inventory() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        1_000,
        45.0,
    ));
    seed_venue_spendability(&mut feed, 1_050);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            50.0
        ))
        .is_some()
    );
    assert!(
        feed.seed_cache_snapshot(
            Vec::<String>::new(),
            Decimal::new(7, 0),
            Decimal::new(2, 0),
            1_200
        )
        .is_some()
    );

    let state = admission
        .capital_admission_state_snapshot()
        .expect("cache seed should publish configured product inventory");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.source, "nt_position_cache");
    assert_eq!(product.observed_at_ns, 1_200);
    assert_eq!(product.yes_position, Decimal::new(7, 0));
    assert_eq!(product.no_position, Decimal::new(2, 0));
}

#[test]
fn cache_seed_and_concurrent_order_event_do_not_double_count() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        1_000,
        45.0,
    ));
    seed_venue_spendability(&mut feed, 1_050);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            50.0
        ))
        .is_some()
    );

    let _ = feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-A",
        1_200,
        AccountId::from("ACCOUNT-001"),
    )));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("live order event should publish lifecycle state");
    assert_eq!(state.order_lifecycle.open_order_count, 1);

    assert!(
        feed.seed_open_order_cache(vec!["client-order-A".to_string()], 1_300)
            .is_some()
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("cache seed should keep lifecycle state");
    assert_eq!(state.order_lifecycle.open_order_count, 1);

    let _ = feed.on_order_event(&OrderEventAny::Canceled(order_canceled_event(
        "client-order-A",
        1_400,
    )));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("terminal event should publish lifecycle state");
    assert_eq!(state.order_lifecycle.open_order_count, 0);

    assert!(
        feed.seed_open_order_cache(vec!["client-order-A".to_string()], 1_500)
            .is_some()
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("stale cache seed should not resurrect terminal order");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
}

#[test]
fn account_bound_live_order_events_update_open_order_count() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        1_000,
        45.0,
    ));
    seed_venue_spendability(&mut feed, 1_050);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            50.0
        ))
        .is_some()
    );
    assert!(
        feed.seed_open_order_cache(Vec::<String>::new(), 1_120)
            .is_some()
    );
    rebuild_empty_capital_admission(&admission);
    let mut request = capital_admission_submit_request("client-order-1");
    request.instrument_id = "instrument-yes.VENUE-A".to_string();
    admission
        .admit_at(&request, 1_150)
        .expect("fresh sizing state should admit")
        .commit_submitted();
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let _ = feed.on_order_event(&OrderEventAny::Submitted(order_submitted_event(
        "client-order-1",
        1_200,
        AccountId::from("ACCOUNT-001"),
    )));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("submitted event should publish lifecycle state");
    assert_eq!(state.order_lifecycle.open_order_count, 1);

    let decision = feed
        .on_order_event(&OrderEventAny::Canceled(order_canceled_event(
            "client-order-1",
            1_300,
        )))
        .expect("terminal event should release matching reservation");

    assert!(decision.accepted);
    assert!(!decision.unknown_reservation);
    let state = admission
        .capital_admission_state_snapshot()
        .expect("terminal event should publish lifecycle state");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn live_order_event_for_submit_owned_reservation_keeps_second_submit_open() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        1_000,
        45.0,
    ));
    seed_venue_spendability(&mut feed, 1_050);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            50.0
        ))
        .is_some()
    );
    assert!(
        feed.seed_open_order_cache(Vec::<String>::new(), 1_120)
            .is_some()
    );
    rebuild_empty_capital_admission(&admission);

    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_150)
        .expect("first fresh submit should admit")
        .commit_submitted();
    let _ = feed.on_order_event(&OrderEventAny::Submitted(order_submitted_event(
        "client-order-1",
        1_200,
        AccountId::from("ACCOUNT-001"),
    )));

    let state = admission
        .capital_admission_state_snapshot()
        .expect("submitted event should publish lifecycle state");
    assert_eq!(state.order_lifecycle.open_order_count, 1);
    assert!(state.order_lifecycle.all_open_orders_attributed);

    admission
        .admit_at(&capital_admission_submit_request("client-order-2"), 1_250)
        .expect("submit-owned live order must not close admission for the next order")
        .commit_submitted();

    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(86, 1))
    );
}

#[test]
fn partial_fill_event_revalues_residual_reservation() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        AccountId::from("ACCOUNT-001"),
    )));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("accepted order should publish lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 1);

    let decision = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("matching fill should update residual liability");

    assert!(decision.accepted);
    assert_eq!(decision.action, CapitalAdmissionLifecycleAction::Revalued);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("partial fill should keep live lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 1);
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.source, "nt_order_fill");
    assert_eq!(product.observed_at_ns, 1_100);
    assert_eq!(product.yes_position, Decimal::new(4, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::new(4, 0));
}

#[test]
fn unknown_external_fill_updates_product_position_once_without_reservation_lifecycle() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-1",
            "external-trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(3),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );

    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("unknown external fill should still publish observed product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.source, "nt_order_fill");
    assert_eq!(product.observed_at_ns, 1_100);
    assert_eq!(product.yes_position, Decimal::new(3, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::new(3, 0));

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-1",
            "external-trade-1",
            1_200,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(3),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("duplicate external trade id should not mutate product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.observed_at_ns, 1_100);
    assert_eq!(product.yes_position, Decimal::new(3, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::new(3, 0));
}

#[test]
fn external_fill_replay_after_dedupe_retention_expires_does_not_double_count_product_position() {
    let (admission, mut feed) = committed_submit_runtime_feed();

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-1",
            "external-trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(3),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-1",
            "external-trade-1",
            1_700,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(3),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );

    let state = admission
        .capital_admission_state_snapshot()
        .expect("post-retention duplicate external fill should leave product state intact");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.observed_at_ns, 1_100);
    assert_eq!(product.yes_position, Decimal::new(3, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::new(3, 0));
}

#[test]
fn authoritative_reseed_rearms_external_fill_accounting_after_retention_latch() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_venue_spendability(&mut feed, 950);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            975,
            100.0,
        ))
        .is_some()
    );
    assert!(
        feed.seed_cache_snapshot(Vec::<String>::new(), Decimal::ZERO, Decimal::ZERO, 1_000)
            .is_some()
    );
    rebuild_empty_capital_admission(&admission);

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-1",
            "external-trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(3),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-1",
            "external-trade-1",
            1_700,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(3),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-2",
            "external-trade-2",
            1_800,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(2),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("latched external fill should preserve pre-reseed product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.observed_at_ns, 1_100);
    assert_eq!(product.yes_position, Decimal::new(3, 0));

    assert!(
        feed.seed_cache_snapshot(Vec::<String>::new(), Decimal::ZERO, Decimal::ZERO, 1_900)
            .is_some()
    );
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-2",
            "external-trade-2",
            2_000,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(2),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );

    let state = admission
        .capital_admission_state_snapshot()
        .expect("authoritative reseed should re-arm external fill accounting");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.observed_at_ns, 2_000);
    assert_eq!(product.yes_position, Decimal::new(2, 0));
}

#[test]
fn known_fill_retention_expiry_does_not_block_distinct_external_fill() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        AccountId::from("ACCOUNT-001"),
    )));

    feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
        "client-order-1",
        "known-trade-1",
        1_100,
        AccountId::from("ACCOUNT-001"),
        Quantity::from(10),
        OrderSide::Buy,
        InstrumentId::from("instrument-yes.VENUE-A"),
    )))
    .expect("matching known fill should release reservation");

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-1",
            "external-trade-1",
            1_700,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(2),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );

    let state = admission
        .capital_admission_state_snapshot()
        .expect("distinct external fill should update after known-fill retention expiry");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.observed_at_ns, 1_700);
    assert_eq!(product.yes_position, Decimal::new(12, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::new(12, 0));
}

#[test]
fn known_fill_replay_after_dedupe_retention_expires_does_not_apply_external_delta() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        AccountId::from("ACCOUNT-001"),
    )));

    feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
        "client-order-1",
        "known-trade-1",
        1_100,
        AccountId::from("ACCOUNT-001"),
        Quantity::from(10),
        OrderSide::Buy,
        InstrumentId::from("instrument-yes.VENUE-A"),
    )))
    .expect("matching known fill should release reservation");

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "known-trade-1",
            1_700,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(10),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );

    let state = admission
        .capital_admission_state_snapshot()
        .expect("post-retention known duplicate should preserve product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.observed_at_ns, 1_100);
    assert_eq!(product.yes_position, Decimal::new(10, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::new(10, 0));
}

#[test]
fn external_fill_dedupe_keys_by_instrument_and_trade_id() {
    let (admission, mut feed) = committed_submit_runtime_feed();

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-1",
            "shared-trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(3),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-2",
            "shared-trade-1",
            1_200,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(2),
            OrderSide::Buy,
            InstrumentId::from("instrument-no.VENUE-A"),
        )))
        .is_none()
    );

    let state = admission
        .capital_admission_state_snapshot()
        .expect("same venue trade id on a different instrument should still publish");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.observed_at_ns, 1_200);
    assert_eq!(product.yes_position, Decimal::new(3, 0));
    assert_eq!(product.no_position, Decimal::new(2, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::new(5, 0));
}

#[test]
fn unknown_reconciliation_fill_does_not_replay_seeded_product_position() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_venue_spendability(&mut feed, 925);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    assert!(
        feed.seed_cache_snapshot(
            Vec::<String>::new(),
            Decimal::new(3, 0),
            Decimal::ZERO,
            1_000
        )
        .is_some()
    );
    rebuild_empty_capital_admission(&admission);

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(
            order_filled_event_with_reconciliation(
                "external-order-1",
                "external-trade-1",
                1_100,
                AccountId::from("ACCOUNT-001"),
                Quantity::from(3),
                OrderSide::Buy,
                InstrumentId::from("instrument-yes.VENUE-A"),
                true,
            )
        ))
        .is_none()
    );

    let state = admission
        .capital_admission_state_snapshot()
        .expect("reconciliation fill should preserve seeded product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_ne!(product.source, "nt_order_fill");
    assert_eq!(product.yes_position, Decimal::new(3, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::ZERO);
}

#[test]
fn full_fill_event_releases_reservation_and_closes_live_order_count() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        AccountId::from("ACCOUNT-001"),
    )));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("accepted order should publish lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 1);

    let decision = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(10),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("matching full fill should release reservation");

    assert!(decision.accepted);
    assert_eq!(decision.action, CapitalAdmissionLifecycleAction::Released);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("full fill should publish lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
    assert_eq!(feed.latest_terminal_observed_at_ns(), Some(1_100));
}

#[test]
fn duplicate_known_full_fill_trade_id_does_not_apply_external_delta_after_release() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        AccountId::from("ACCOUNT-001"),
    )));

    feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
        "client-order-1",
        "trade-1",
        1_100,
        AccountId::from("ACCOUNT-001"),
        Quantity::from(10),
        OrderSide::Buy,
        InstrumentId::from("instrument-yes.VENUE-A"),
    )))
    .expect("matching full fill should release reservation");

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_200,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(10),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );

    let state = admission
        .capital_admission_state_snapshot()
        .expect("duplicate known trade id should not mutate product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.observed_at_ns, 1_100);
    assert_eq!(product.yes_position, Decimal::new(10, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::new(10, 0));
}

#[test]
fn distinct_post_cancel_fill_updates_product_position_after_terminal_release() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        AccountId::from("ACCOUNT-001"),
    )));

    let partial = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("partial fill should revalue reservation");
    assert!(partial.accepted);
    assert_eq!(partial.action, CapitalAdmissionLifecycleAction::Revalued);

    let terminal = feed
        .on_order_event(&OrderEventAny::Canceled(order_canceled_event(
            "client-order-1",
            1_200,
        )))
        .expect("cancel should release residual reservation");
    assert!(terminal.accepted);
    assert_eq!(terminal.action, CapitalAdmissionLifecycleAction::Released);

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_300,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("post-terminal duplicate fill should not mutate product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.observed_at_ns, 1_100);
    assert_eq!(product.yes_position, Decimal::new(4, 0));

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-2",
            1_400,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(2),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("distinct post-terminal fill should publish product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.source, "nt_order_fill");
    assert_eq!(product.observed_at_ns, 1_400);
    assert_eq!(product.yes_position, Decimal::new(6, 0));
    assert_eq!(product.conditional_token_allowance, Decimal::new(6, 0));
}

#[test]
fn sell_fill_event_reduces_inventory_before_next_sell_admission() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut config = runtime_feed_config();
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = &mut config.product_state;
    product.conditional_token_allowance = Decimal::new(10, 0);
    let mut feed = CapitalAdmissionRuntimeFeed::new(config, admission.clone());
    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_venue_spendability(&mut feed, 925);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    assert!(
        feed.seed_cache_snapshot(
            Vec::<String>::new(),
            Decimal::new(10, 0),
            Decimal::ZERO,
            1_000
        )
        .is_some()
    );
    rebuild_empty_capital_admission(&admission);

    admission
        .admit_at(
            &capital_admission_sell_submit_request("client-order-1"),
            1_010,
        )
        .expect("sell within seeded YES inventory should admit")
        .commit_submitted();
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(30, 2))
    );
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        AccountId::from("ACCOUNT-001"),
    )));
    let decision = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(10),
            OrderSide::Sell,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("matching sell fill should release the first order");
    assert_eq!(decision.action, CapitalAdmissionLifecycleAction::Released);

    let state = admission
        .capital_admission_state_snapshot()
        .expect("sell fill should publish updated inventory");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.source, "nt_order_fill");
    assert_eq!(product.observed_at_ns, 1_100);
    assert_eq!(product.yes_position, Decimal::ZERO);
    assert_eq!(product.conditional_token_allowance, Decimal::ZERO);

    let second = admission
        .admit_at(
            &capital_admission_sell_submit_request("client-order-2"),
            1_150,
        )
        .expect_err("sell above post-fill inventory should reject");
    assert_eq!(
        second,
        BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
            reason: BoltV3CapitalAdmissionRejectReason::CapitalAdmissionRejected,
        }
    );
}

#[test]
fn fill_event_account_or_instrument_mismatch_is_non_mutating() {
    let (admission, mut feed) = committed_submit_runtime_feed();
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
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-other.VENUE-A"),
        )))
        .is_none()
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}

#[test]
fn fill_event_for_rebuilt_reservation_revalues_residual() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_venue_spendability(&mut feed, 925);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    let rebuild = admission.rebuild_capital_admission_open_order_reservations(
        vec![open_order_reservation(
            "client-order-1",
            "client-order-1#rebuilt",
            Decimal::new(43, 1),
        )],
        1_000,
    );
    assert!(rebuild.accepted);

    let decision = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("rebuilt reservation metadata should support residual revalue");
    assert!(decision.accepted);
    assert_eq!(decision.action, CapitalAdmissionLifecycleAction::Revalued);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
    assert_eq!(feed.latest_terminal_observed_at_ns(), None);
}

#[test]
fn reconciliation_fill_for_recovered_startup_reservation_is_idempotent() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_venue_spendability(&mut feed, 925);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    let mut reservation = open_order_reservation(
        "client-order-1",
        "client-order-1#rebuilt",
        Decimal::new(43, 1),
    );
    reservation.recovered_from_startup = true;
    let rebuild =
        admission.rebuild_capital_admission_open_order_reservations(vec![reservation], 1_000);
    assert!(rebuild.accepted);

    let reconciliation = feed
        .on_order_event(&OrderEventAny::Filled(
            order_filled_event_with_reconciliation(
                "client-order-1",
                "trade-1",
                1_100,
                AccountId::from("ACCOUNT-001"),
                Quantity::from(4),
                OrderSide::Buy,
                InstrumentId::from("instrument-yes.VENUE-A"),
                true,
            ),
        ))
        .expect("startup reconciliation fill should be accepted as already accounted");
    assert!(reconciliation.accepted);
    assert_eq!(reconciliation.action, CapitalAdmissionLifecycleAction::None);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let duplicate = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_200,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("seen reconciliation trade id should stay idempotent");
    assert!(duplicate.accepted);
    assert_eq!(duplicate.action, CapitalAdmissionLifecycleAction::None);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let terminal = feed
        .on_order_event(&OrderEventAny::Canceled(order_canceled_event(
            "client-order-1",
            1_300,
        )))
        .expect("terminal event should release recovered startup reservation");
    assert!(terminal.accepted);
    assert_eq!(terminal.action, CapitalAdmissionLifecycleAction::Released);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_400,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none()
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("post-terminal duplicate reconciliation fill should not mutate product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_ne!(product.source, "nt_order_fill");
    assert_eq!(product.yes_position, Decimal::ZERO);
}

#[test]
fn attributed_rebuild_after_cache_seed_keeps_next_submit_open() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_venue_spendability(&mut feed, 925);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    assert!(
        feed.seed_open_order_cache(vec!["client-order-1".to_string()], 1_000)
            .is_some()
    );

    let rebuild = admission.rebuild_capital_admission_open_order_reservations(
        vec![open_order_reservation(
            "client-order-1",
            "client-order-1#rebuilt",
            Decimal::new(43, 1),
        )],
        1_000,
    );
    assert!(rebuild.accepted);

    let state = admission
        .capital_admission_state_snapshot()
        .expect("attributed rebuild should retain NT state");
    assert_eq!(state.order_lifecycle.open_order_count, 1);
    assert!(state.order_lifecycle.all_open_orders_attributed);

    admission
        .admit_at(&capital_admission_submit_request("client-order-2"), 1_100)
        .expect("attributed startup order should not close later submits")
        .commit_submitted();
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(86, 1))
    );
}

#[test]
fn account_refresh_after_attributed_rebuild_preserves_order_lifecycle_attribution() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_venue_spendability(&mut feed, 925);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    assert!(
        feed.seed_open_order_cache(vec!["client-order-1".to_string()], 1_000)
            .is_some()
    );

    let rebuild = admission.rebuild_capital_admission_open_order_reservations(
        vec![open_order_reservation(
            "client-order-1",
            "client-order-1#rebuilt",
            Decimal::new(43, 1),
        )],
        1_000,
    );
    assert!(rebuild.accepted);

    assert!(
        feed.on_account_state(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_050,
            100.0,
        ))
        .is_some()
    );

    let state = admission
        .capital_admission_state_snapshot()
        .expect("account refresh should preserve rebuilt NT state");
    assert_eq!(state.order_lifecycle.open_order_count, 1);
    assert!(state.order_lifecycle.all_open_orders_attributed);

    admission
        .admit_at(&capital_admission_submit_request("client-order-2"), 1_100)
        .expect("account refresh should not erase attributed startup rebuild")
        .commit_submitted();
    let _ = feed.on_order_event(&OrderEventAny::Submitted(order_submitted_event(
        "client-order-2",
        1_150,
        AccountId::from("ACCOUNT-001"),
    )));

    let state = admission
        .capital_admission_state_snapshot()
        .expect("second submit event should preserve rebuilt NT state");
    assert_eq!(state.order_lifecycle.open_order_count, 2);
    assert!(state.order_lifecycle.all_open_orders_attributed);

    let mut third_request = capital_admission_submit_request("client-order-3");
    third_request.economics_admission =
        support::sample_economics_admission_with_debit(Decimal::new(4, 1), Decimal::new(3, 1));
    third_request.notional = Decimal::new(7, 1);
    third_request.order_quantity = Decimal::new(1, 0);
    third_request
        .admission_evidence
        .as_mut()
        .expect("third request should carry admission evidence")
        .quantity = Decimal::new(1, 0);
    admission
        .admit_at(&third_request, 1_200)
        .expect("submitted event should not erase attributed startup rebuild")
        .commit_submitted();
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(930, 2))
    );
}

#[test]
fn full_fill_event_for_rebuilt_reservation_releases_and_closes_live_order_count() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_venue_spendability(&mut feed, 925);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    assert!(
        feed.seed_open_order_cache(vec!["client-order-1".to_string()], 1_000)
            .is_some()
    );
    let rebuild = admission.rebuild_capital_admission_open_order_reservations(
        vec![open_order_reservation(
            "client-order-1",
            "client-order-1#rebuilt",
            Decimal::new(43, 1),
        )],
        1_000,
    );
    assert!(rebuild.accepted);

    let decision = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(10),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("rebuilt reservation full fill should release");

    assert!(decision.accepted);
    assert_eq!(decision.action, CapitalAdmissionLifecycleAction::Released);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    assert_eq!(feed.latest_terminal_observed_at_ns(), Some(1_100));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("fill release should publish updated lifecycle state");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
    assert!(state.order_lifecycle.all_open_orders_attributed);
}

#[test]
fn duplicate_fill_trade_id_with_different_runtime_instrument_is_non_mutating() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
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
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-no.VENUE-A"),
        )))
        .is_none()
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}

#[test]
fn terminal_event_after_partial_fill_releases_residual_reservation() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-1",
        1_050,
        AccountId::from("ACCOUNT-001"),
    )));
    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
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

    assert_eq!(terminal.action, CapitalAdmissionLifecycleAction::Released);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("terminal should publish lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
}

#[test]
fn terminal_nt_order_event_releases_committed_submit_reservation() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let decision = feed
        .on_order_event(&OrderEventAny::Canceled(order_canceled_event(
            "client-order-1",
            1_100,
        )))
        .expect("terminal event for configured account should produce lifecycle decision");

    assert!(decision.accepted);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    assert_eq!(
        feed.latest_terminal_observed_at_ns(),
        Some(1_100),
        "feed should expose latest accepted terminal NT event timestamp"
    );
}

#[test]
fn configured_submit_sizer_rejects_stale_venue_spendability_before_nt_submit() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut components = fresh_components(900);
    components.venue_spendability.observed_at_ns = 100;
    admission.update_capital_admission_nt_components(components);
    rebuild_empty_capital_admission(&admission);

    let error = admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect_err("stale venue spendability evidence must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
            reason: BoltV3CapitalAdmissionRejectReason::StaleNtState
        }
    ));
}

#[test]
fn subscribed_terminal_nt_order_event_releases_committed_submit_reservation() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let feed = Arc::new(Mutex::new(CapitalAdmissionRuntimeFeed::new(
        runtime_feed_config(),
        admission.clone(),
    )));
    let mut subscription = subscribe_capital_admission_runtime_feed(feed.clone());

    publish_order_event(
        switchboard::get_event_order_topic(StrategyId::from("strategy-a")),
        &OrderEventAny::Canceled(order_canceled_event("client-order-1", 1_100)),
    );
    subscription.unsubscribe_all();

    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
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
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let decision = feed
        .on_order_event(&OrderEventAny::Denied(order_denied_event(
            "client-order-1",
            1_100,
        )))
        .expect("account-less denied event should be matched by committed reservation id");

    assert!(decision.accepted);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
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
            AccountId::from("ACCOUNT-001"),
        )),
    );
    assert_terminal_event_releases(
        "client-order-expired",
        OrderEventAny::Expired(order_expired_event(
            "client-order-expired",
            1_200,
            Some(AccountId::from("ACCOUNT-001")),
        )),
    );
}

#[test]
fn account_bound_terminal_nt_order_event_for_other_account_is_ignored() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.on_order_event(&OrderEventAny::Rejected(order_rejected_event(
            "client-order-1",
            1_100,
            AccountId::from("OTHER-ACCOUNT"),
        )))
        .is_none()
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    assert_eq!(feed.latest_terminal_observed_at_ns(), None);
}

#[test]
fn forced_reduction_terminal_nt_order_event_releases_live_forced_reduction_cap() {
    let admission = Arc::new(capital_admission_configured_admission());
    admission.replace_kill_switch_state(KillSwitchState::Flattening {
        halt_id: "halt-001".to_string(),
    });
    admission.configure_kill_switch_forced_reduction_policy(forced_reduction_policy());
    let first = forced_reduction_submit_request("halt-001", "forced-reduction-1");
    let second = forced_reduction_submit_request("halt-001", "forced-reduction-2");
    let third = forced_reduction_submit_request("halt-001", "forced-reduction-3");

    admission
        .admit_at(&first, 1_000)
        .expect("first forced reduction should reserve the live slot")
        .commit_submitted();

    let capped = admission
        .admit_at(&second, 1_050)
        .expect_err("second forced reduction should hit the unreleased live cap");
    assert!(matches!(
        capped,
        BoltV3SubmitAdmissionError::KillSwitchForcedReductionCapExceeded
    ));

    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    assert!(
        feed.on_order_event(&OrderEventAny::Canceled(order_canceled_event(
            "forced-reduction-1",
            1_100,
        )))
        .is_none(),
        "forced-reduction terminal release is independent of capital-reservation ownership"
    );

    admission
        .admit_at(&second, 1_200)
        .expect("terminal forced-reduction order event should release the live cap")
        .commit_submitted();

    assert!(
        feed.on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "forced-reduction-2",
            "forced-reduction-trade-2",
            1_300,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(1),
            OrderSide::Sell,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .is_none(),
        "filled forced-reduction terminals also release outside capital ownership"
    );

    admission
        .admit_at(&third, 1_400)
        .expect("filled forced-reduction order event should release the live cap")
        .commit_submitted();
}

#[test]
fn account_less_non_denied_terminal_nt_order_event_is_ignored() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.on_order_event(&OrderEventAny::Expired(order_expired_event(
            "client-order-1",
            1_100,
            None,
        )))
        .is_none()
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    assert_eq!(feed.latest_terminal_observed_at_ns(), None);
}

fn assert_terminal_event_releases(client_order_id: &str, event: OrderEventAny) {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request(client_order_id), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let observed_at_ns = event.ts_event().as_u64();
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let decision = feed
        .on_order_event(&event)
        .expect("terminal event should release matching committed reservation");

    assert!(decision.accepted);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    assert_eq!(feed.latest_terminal_observed_at_ns(), Some(observed_at_ns));
}

fn runtime_feed_config() -> CapitalAdmissionRuntimeFeedConfig {
    CapitalAdmissionRuntimeFeedConfig {
        venue_id: "VENUE-A".to_string(),
        account_id: AccountId::from("ACCOUNT-001"),
        collateral_currency: "USD".to_string(),
        product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
            PredictionMarketAdmissionSnapshot {
                source: "bolt_configured_binary_product".to_string(),
                observed_at_ns: 900,
                yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                no_instrument_id: "instrument-no.VENUE-A".to_string(),
                yes_position: Decimal::ZERO,
                no_position: Decimal::ZERO,
                collateral_allowance: Decimal::ZERO,
                conditional_token_allowance: Decimal::ZERO,
                collateral_coupled_group_id: "group-1".to_string(),
            },
        ),
        startup_observed_at_ns: 900,
        dedupe_retention_ns: 500,
    }
}

fn polymarket_runtime_feed_config() -> CapitalAdmissionRuntimeFeedConfig {
    let mut config = runtime_feed_config();
    config.venue_id = "POLYMARKET".to_string();
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = &mut config.product_state;
    product.yes_instrument_id = "condition-yes123.POLYMARKET".to_string();
    product.no_instrument_id = "condition-no456.POLYMARKET".to_string();
    config
}

fn account_state(
    account_id: AccountId,
    currency_code: &str,
    ts_event: u64,
    free_collateral: f64,
) -> AccountState {
    let currency = test_currency(currency_code);
    AccountState::new(
        account_id,
        AccountType::Cash,
        vec![AccountBalance::new(
            Money::new(free_collateral, currency),
            Money::new(0.0, currency),
            Money::new(free_collateral, currency),
        )],
        vec![],
        true,
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        Some(currency),
    )
}

fn portfolio_snapshot(
    account_id: AccountId,
    currency_code: &str,
    ts_event: u64,
    total_equity: f64,
) -> PortfolioSnapshot {
    let currency = test_currency(currency_code);
    PortfolioSnapshot::new(
        account_id,
        AccountType::Cash,
        Some(currency),
        vec![],
        vec![],
        vec![],
        vec![],
        vec![Money::new(total_equity, currency)],
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
    )
}

fn adjusted_position_event(account_id: AccountId, ts_event: u64) -> PositionEvent {
    PositionEvent::PositionAdjusted(PositionAdjusted::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from("instrument-yes.VENUE-A"),
        PositionId::from("position-1"),
        account_id,
        PositionAdjustmentType::Commission,
        None,
        Some(Money::new(0.0, test_currency("USD"))),
        None,
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
    ))
}

fn test_currency(currency_code: &str) -> Currency {
    if currency_code == "USD" {
        return Currency::new("USD", 2, 0, "Test USD", CurrencyType::Fiat);
    }
    Currency::from(currency_code)
}

fn poisoned_capital_admission_runtime_feed() -> Arc<Mutex<CapitalAdmissionRuntimeFeed>> {
    let admission = Arc::new(capital_admission_configured_admission());
    let feed = Arc::new(Mutex::new(CapitalAdmissionRuntimeFeed::new(
        runtime_feed_config(),
        admission,
    )));
    poison_lock(&feed);
    feed
}

fn poison_lock<T>(lock: &Arc<Mutex<T>>) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let _g = lock.lock().unwrap();
        panic!("seed poison");
    }));
}

fn capital_admission_configured_admission() -> BoltV3SubmitAdmissionState {
    capital_admission_configured_admission_with_writer(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ))
}

fn capital_admission_configured_admission_with_writer(
    writer: Arc<dyn bolt_v2::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
) -> BoltV3SubmitAdmissionState {
    capital_admission_configured_admission_with_writer_and_venue(writer, "VENUE-A")
}

fn polymarket_capital_admission_configured_admission() -> BoltV3SubmitAdmissionState {
    capital_admission_configured_admission_with_writer_and_venue(
        Arc::new(support::RecordingDecisionEvidenceWriter::default()),
        "POLYMARKET",
    )
}

fn capital_admission_configured_admission_with_writer_and_venue(
    writer: Arc<dyn bolt_v2::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
    venue_id: &str,
) -> BoltV3SubmitAdmissionState {
    BoltV3SubmitAdmissionState::new_with_capital_admission(
        writer,
        BoltV3SubmitCapitalAdmissionConfig {
            venue_id: venue_id.to_string(),
            account_id: "ACCOUNT-001".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            collateral_currency: "USD".to_string(),
            capital_pool: CapitalPoolSnapshot {
                source: "bolt_submit_sizer_bootstrap".to_string(),
                observed_at_ns: 900,
                pool_id: "pool-1".to_string(),
                max_pool_liability: Decimal::new(10, 0),
                committed_liability: Decimal::ZERO,
                max_snapshot_age_ns: 500,
            },
            policy: CapitalAdmissionPolicy {
                min_remaining_pool_balance: None,
            },
            dedupe_retention_ns: 500,
        },
    )
}

fn arm_default(_admission: &BoltV3SubmitAdmissionState) {}

fn rebuild_empty_capital_admission(admission: &BoltV3SubmitAdmissionState) {
    let rebuild = admission.rebuild_capital_admission_open_order_reservations(Vec::new(), 1_000);
    assert!(
        rebuild.accepted,
        "test startup rebuild should open submit admission"
    );
    assert_eq!(admission.capital_admission_reconciled(), Some(true));
}

fn open_order_reservation(
    client_order_id: &str,
    submit_reservation_id: &str,
    liability: Decimal,
) -> BoltV3SubmitCapitalAdmissionOpenOrderReservation {
    BoltV3SubmitCapitalAdmissionOpenOrderReservation {
        client_order_id: client_order_id.to_string(),
        submit_reservation_id: submit_reservation_id.to_string(),
        collateral_group_id: "group-1".to_string(),
        liability,
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        side: BoltV3CompiledOrderSide::Buy,
        open_quantity: Decimal::new(10, 0),
        original_quantity: Decimal::new(10, 0),
        filled_quantity: Decimal::ZERO,
        liability_factor: Decimal::new(4, 1),
        additive_liability: Decimal::new(3, 1),
        seen_trade_ids: Default::default(),
        recovered_from_startup: false,
        observed_at_ns: 1_000,
        evidence_label: "nt_open_order_cache".to_string(),
    }
}

fn committed_submit_runtime_feed() -> (Arc<BoltV3SubmitAdmissionState>, CapitalAdmissionRuntimeFeed)
{
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.on_account_state(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_venue_spendability(&mut feed, 925);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    assert!(
        feed.seed_open_order_cache(Vec::<String>::new(), 1_000)
            .is_some()
    );
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect("fresh capital admission state and capacity should admit")
        .commit_submitted();
    (admission, feed)
}

fn capital_admission_submit_request(client_order_id: &str) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        economics_admission: support::sample_economics_admission_with_debit(
            Decimal::new(4, 0),
            Decimal::new(3, 1),
        ),
        strategy_id: "strategy-a".to_string(),
        execution_client_id: "execution-client-a".to_string(),
        client_order_id: client_order_id.to_string(),
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        notional: Decimal::new(43, 1),
        order_side: OrderSide::Buy,
        order_quantity: Decimal::new(10, 0),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_proof: None,
        kill_switch_forced_reduction: None,
        admission_evidence: Some(BoltV3CompiledOrderAdmissionEvidence {
            venue_id: "VENUE-A".to_string(),
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

fn forced_reduction_policy() -> BoltV3KillSwitchForcedReductionPolicy {
    BoltV3KillSwitchForcedReductionPolicy::new(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        1,
        Decimal::new(10, 0),
    )
    .expect("forced-reduction policy should be valid")
}

fn forced_reduction_claim(halt_id: &str) -> BoltV3KillSwitchForcedReductionClaim {
    BoltV3KillSwitchForcedReductionClaim::new(
        halt_id,
        "flatten-positions",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .expect("forced-reduction claim should be valid")
}

fn forced_reduction_submit_request(
    halt_id: &str,
    client_order_id: &str,
) -> BoltV3SubmitAdmissionRequest {
    let mut request = capital_admission_sell_submit_request(client_order_id);
    request.notional = Decimal::new(5, 0);
    request.intent_kind = BoltV3SubmitIntentKind::KillSwitchForcedReduction;
    request.lifecycle_policy = BoltV3SubmitLifecyclePolicy::new(false);
    request.risk_reducing_exit_proof = None;
    request.kill_switch_forced_reduction = Some(forced_reduction_claim(halt_id));
    request
}

fn capital_admission_sell_submit_request(client_order_id: &str) -> BoltV3SubmitAdmissionRequest {
    let mut request = capital_admission_submit_request(client_order_id);
    request.order_side = OrderSide::Sell;
    request
        .admission_evidence
        .as_mut()
        .expect("capital admission request should carry evidence")
        .side = BoltV3CompiledOrderSide::Sell;
    request
}

fn risk_reducing_exit_submit_request(client_order_id: &str) -> BoltV3SubmitAdmissionRequest {
    let mut request = capital_admission_sell_submit_request(client_order_id);
    request.intent_kind = BoltV3SubmitIntentKind::RiskReducingExit;
    request.risk_reducing_exit_proof = Some(BoltV3RiskReducingExitProof {
        position_id: "position-1".to_string(),
        instrument_id: request.instrument_id.clone(),
        position_side: PositionSide::Long,
        exit_order_side: request.order_side,
        position_quantity: request.order_quantity,
        exit_quantity: request.order_quantity,
    });
    request
}

fn fresh_capital_admission_state(observed_at_ns: u64) -> NtDerivedCapitalAdmissionState {
    NtDerivedCapitalAdmissionState {
        source: "nt_capital_admission_state".to_string(),
        observed_at_ns,
        portfolio: PortfolioCapitalAdmissionSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns,
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            collateral_currency: "USD".to_string(),
            free_collateral: Decimal::new(100, 0),
            total_equity: Decimal::new(100, 0),
        },
        venue_spendability: venue_spendability_snapshot(observed_at_ns, 100, 100),
        order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
            source: "nt_open_order_cache".to_string(),
            observed_at_ns,
            open_order_count: 0,
            all_open_orders_attributed: true,
        },
        product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
            PredictionMarketAdmissionSnapshot {
                source: "nt_prediction_market_snapshot".to_string(),
                observed_at_ns,
                yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                no_instrument_id: "instrument-no.VENUE-A".to_string(),
                yes_position: Decimal::new(10, 0),
                no_position: Decimal::ZERO,
                collateral_allowance: Decimal::new(100, 0),
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

fn fresh_components(observed_at_ns: u64) -> BoltV3SubmitCapitalAdmissionNtComponents {
    let state = fresh_capital_admission_state(observed_at_ns);
    BoltV3SubmitCapitalAdmissionNtComponents {
        source: state.source,
        observed_at_ns: state.observed_at_ns,
        portfolio: state.portfolio,
        venue_spendability: state.venue_spendability,
        order_lifecycle: state.order_lifecycle,
        product_state: state.product_state,
        loss_snapshot: state.loss_snapshot,
    }
}

fn seed_venue_spendability(feed: &mut CapitalAdmissionRuntimeFeed, observed_at_ns: u64) {
    let _ =
        feed.on_venue_spendability_snapshot(venue_spendability_snapshot(observed_at_ns, 100, 100));
}

fn venue_spendability_snapshot(
    observed_at_ns: u64,
    spendable_collateral: i64,
    collateral_allowance: i64,
) -> VenueSpendabilitySnapshot {
    VenueSpendabilitySnapshot {
        source: "operator-venue-spendability".to_string(),
        observed_at_ns,
        venue_id: "VENUE-A".to_string(),
        account_id: "ACCOUNT-001".to_string(),
        collateral_currency: "USD".to_string(),
        spendable_collateral: Decimal::new(spendable_collateral, 0),
        collateral_allowance: Decimal::new(collateral_allowance, 0),
    }
}

fn polymarket_venue_truth_snapshot(
    captured_at: u64,
    balance: Decimal,
    allowance: Decimal,
) -> VenueTruthSnapshot {
    build_polymarket_venue_truth_snapshot(PolymarketVenueTruthInput {
        captured_at: UnixNanos::from(captured_at),
        account_id: AccountId::from("ACCOUNT-001"),
        collateral_currency: Currency::from("USD"),
        collateral: BalanceAllowance {
            balance,
            allowance: Some(allowance),
        },
        open_orders: Vec::new(),
        positions: Vec::new(),
    })
    .expect("test venue truth snapshot should be valid")
}

fn polymarket_venue_truth_snapshot_with_orders_and_positions(
    captured_at: u64,
    balance: Decimal,
    allowance: Decimal,
) -> VenueTruthSnapshot {
    build_polymarket_venue_truth_snapshot(PolymarketVenueTruthInput {
        captured_at: UnixNanos::from(captured_at),
        account_id: AccountId::from("ACCOUNT-001"),
        collateral_currency: Currency::from("USD"),
        collateral: BalanceAllowance {
            balance,
            allowance: Some(allowance),
        },
        open_orders: vec![polymarket_open_order(
            "venue-order-1",
            "condition",
            "yes123",
            Decimal::new(10, 0),
            Decimal::new(4, 0),
        )],
        positions: vec![
            DataApiPosition {
                asset: "yes123".to_string(),
                condition_id: "condition".to_string(),
                size: 7.0,
                avg_price: Some(0.42),
            },
            DataApiPosition {
                asset: "no456".to_string(),
                condition_id: "condition".to_string(),
                size: 2.0,
                avg_price: Some(0.58),
            },
        ],
    })
    .expect("test venue truth snapshot should be valid")
}

fn polymarket_venue_truth_snapshot_with_position(
    captured_at: u64,
    balance: Decimal,
    allowance: Decimal,
    asset: &str,
    size: f64,
) -> VenueTruthSnapshot {
    build_polymarket_venue_truth_snapshot(PolymarketVenueTruthInput {
        captured_at: UnixNanos::from(captured_at),
        account_id: AccountId::from("ACCOUNT-001"),
        collateral_currency: Currency::from("USD"),
        collateral: BalanceAllowance {
            balance,
            allowance: Some(allowance),
        },
        open_orders: Vec::new(),
        positions: vec![DataApiPosition {
            asset: asset.to_string(),
            condition_id: "condition".to_string(),
            size,
            avg_price: Some(0.42),
        }],
    })
    .expect("test venue truth snapshot should be valid")
}

fn polymarket_open_order(
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

fn order_canceled_event(client_order_id: &str, ts_event: u64) -> OrderCanceled {
    OrderCanceled::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from("instrument-yes.VENUE-A"),
        ClientOrderId::from(client_order_id),
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
        Some(VenueOrderId::from("venue-order-1")),
        Some(AccountId::from("ACCOUNT-001")),
    )
}

fn order_accepted_event(
    client_order_id: &str,
    ts_event: u64,
    account_id: AccountId,
) -> OrderAccepted {
    OrderAccepted::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from("instrument-yes.VENUE-A"),
        ClientOrderId::from(client_order_id),
        VenueOrderId::from("venue-order-1"),
        account_id,
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
    )
}

fn order_submitted_event(
    client_order_id: &str,
    ts_event: u64,
    account_id: AccountId,
) -> OrderSubmitted {
    OrderSubmitted::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from("instrument-yes.VENUE-A"),
        ClientOrderId::from(client_order_id),
        account_id,
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
    )
}

fn order_filled_event_with(
    client_order_id: &str,
    trade_id: &str,
    ts_event: u64,
    account_id: AccountId,
    quantity: Quantity,
    order_side: OrderSide,
    instrument_id: InstrumentId,
) -> OrderFilled {
    order_filled_event_with_reconciliation(
        client_order_id,
        trade_id,
        ts_event,
        account_id,
        quantity,
        order_side,
        instrument_id,
        false,
    )
}

fn order_filled_event_with_reconciliation(
    client_order_id: &str,
    trade_id: &str,
    ts_event: u64,
    account_id: AccountId,
    quantity: Quantity,
    order_side: OrderSide,
    instrument_id: InstrumentId,
    reconciliation: bool,
) -> OrderFilled {
    OrderFilled::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        instrument_id,
        ClientOrderId::from(client_order_id),
        VenueOrderId::from("venue-order-1"),
        account_id,
        TradeId::from(trade_id),
        order_side,
        OrderType::Limit,
        quantity,
        Price::from("0.40"),
        test_currency("USD"),
        LiquiditySide::Taker,
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        reconciliation,
        Some(PositionId::from("position-1")),
        None,
    )
}

fn order_denied_event(client_order_id: &str, ts_event: u64) -> OrderDenied {
    OrderDenied::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from("instrument-yes.VENUE-A"),
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
        InstrumentId::from("instrument-yes.VENUE-A"),
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
        InstrumentId::from("instrument-yes.VENUE-A"),
        ClientOrderId::from(client_order_id),
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
        Some(VenueOrderId::from("venue-order-1")),
        account_id,
    )
}
