use crate::support;

use std::{
    cell::Cell,
    panic::AssertUnwindSafe,
    rc::Rc,
    sync::{Arc, Mutex},
};

use bolt_v2::bolt_v3_capital_admission::{
    CapitalAdmissionPolicy, FeeSlippagePolicy, PredictionMarketAdmissionSnapshot,
    ProductAdmissionSnapshot, ProductKind,
};
use bolt_v2::bolt_v3_capital_admission_runtime_feed::{
    CapitalAdmissionNtCacheProjection, CapitalAdmissionProjectionError,
    CapitalAdmissionRuntimeFeed, CapitalAdmissionRuntimeFeedConfig,
    POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE, subscribe_submit_admission_nt_projection,
};
use bolt_v2::bolt_v3_capital_admission_state::{
    NtDerivedCapitalAdmissionState, OrderLifecycleCapitalAdmissionSnapshot,
    PortfolioCapitalAdmissionSnapshot, ProviderCollateralAllowanceSnapshot,
    ReservationLedgerSnapshot,
};
use bolt_v2::bolt_v3_capital_reservation::{CapitalPoolSnapshot, ReservationRejectionReason};
use bolt_v2::bolt_v3_current_evidence::{
    CapitalAdmissionRebuildSource, DecisionEvidenceRecorder,
    ProviderCollateralAllowanceCaptureEndpoint as EvidenceCaptureEndpoint,
    ProviderCollateralAllowanceCaptureErrorClass as EvidenceCaptureErrorClass,
};
use bolt_v2::bolt_v3_kill_switch::KillSwitchStateKind;
use bolt_v2::bolt_v3_provider_collateral_allowance::{
    ProviderCollateralAllowanceCaptureEndpoint, ProviderCollateralAllowanceCaptureErrorClass,
    ProviderCollateralAllowanceCaptureFailureEvidence,
};
use bolt_v2::bolt_v3_providers::polymarket::{
    PolymarketProviderCollateralAllowanceInput,
    build_polymarket_provider_collateral_allowance_snapshot,
};
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3CapitalAdmissionRejectReason, BoltV3CompiledOrderAdmissionEvidence,
    BoltV3CompiledOrderKind, BoltV3CompiledOrderLiquidity, BoltV3CompiledOrderSide,
    BoltV3CompiledProductKind, BoltV3RiskReducingExitProof, BoltV3SubmitAdmissionError,
    BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitCapitalAdmissionConfig,
    BoltV3SubmitCapitalAdmissionNtComponents, BoltV3SubmitCapitalAdmissionOpenOrderReservation,
    BoltV3SubmitCapitalAdmissionOpenOrderSnapshot, BoltV3SubmitIntentKind,
    PredictionMarketOutcomeSide,
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
        OrderFilled, OrderRejected, PortfolioSnapshot, PositionAdjusted, PositionEvent,
    },
    identifiers::{
        AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId, TradeId, TraderId,
        VenueOrderId,
    },
    types::{AccountBalance, Currency, Money, Price, Quantity},
};
use nautilus_polymarket::http::query::BalanceAllowance;
use rust_decimal::Decimal;
use ustr::Ustr;

fn no_op_nt_projection() -> Rc<dyn Fn()> {
    Rc::new(|| {})
}

trait CapitalAdmissionRuntimeFeedCanonicalNtFixture {
    fn project_account_fixture(
        &mut self,
        account_state: &AccountState,
    ) -> Option<BoltV3SubmitCapitalAdmissionNtComponents>;

    fn project_portfolio_fixture(
        &mut self,
        portfolio_snapshot: &PortfolioSnapshot,
    ) -> Option<BoltV3SubmitCapitalAdmissionNtComponents>;
}

impl CapitalAdmissionRuntimeFeedCanonicalNtFixture for CapitalAdmissionRuntimeFeed {
    fn project_account_fixture(
        &mut self,
        account_state: &AccountState,
    ) -> Option<BoltV3SubmitCapitalAdmissionNtComponents> {
        if account_state.account_id != self.configured_account_id() {
            return None;
        }
        let collateral_currency = self.configured_collateral_currency();
        let balance = account_state
            .balances
            .iter()
            .find(|balance| balance.currency.code.as_str() == collateral_currency)?;
        self.canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: self.accepted_allowance_observed_at_ns(),
            account_balances: Some((balance.free.as_decimal(), balance.total.as_decimal())),
            open_client_order_ids: Vec::new(),
            yes_position: Decimal::ZERO,
            no_position: Decimal::ZERO,
            observed_at_ns: account_state.ts_event.as_u64(),
        })
        .ok()
    }

    fn project_portfolio_fixture(
        &mut self,
        portfolio_snapshot: &PortfolioSnapshot,
    ) -> Option<BoltV3SubmitCapitalAdmissionNtComponents> {
        if portfolio_snapshot.account_id != self.configured_account_id() {
            return None;
        }
        let collateral_currency = self.configured_collateral_currency();
        let total_equity = portfolio_snapshot
            .total_equity
            .iter()
            .find(|money| money.currency.code.as_str() == collateral_currency)
            .map(|money| money.as_decimal())?;
        self.canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: self.accepted_allowance_observed_at_ns(),
            account_balances: Some((total_equity, total_equity)),
            open_client_order_ids: Vec::new(),
            yes_position: Decimal::ZERO,
            no_position: Decimal::ZERO,
            observed_at_ns: portfolio_snapshot.ts_event.as_u64(),
        })
        .ok()
    }
}

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
}

#[test]
fn nt_projection_request_revokes_new_risk_without_erasing_committed_reservation() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect("fresh canonical NT projection should admit")
        .commit_submitted();

    admission.invalidate_capital_admission_for_nt_projection_request();
    admission.invalidate_capital_admission_for_nt_projection_request();

    assert_eq!(admission.capital_admission_reconciled(), Some(false));
    assert!(
        admission.capital_admission_has_live_reservation("client-order-1"),
        "projection invalidation must preserve committed evidence correlation"
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    assert!(matches!(
        admission.admit_at(&capital_admission_submit_request("client-order-2"), 1_001),
        Err(BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
            reason: BoltV3CapitalAdmissionRejectReason::ReconciliationRequired
        })
    ));
}

#[test]
fn stale_nt_projection_candidate_cannot_rearm_after_newer_invalidation() {
    let admission = capital_admission_configured_admission();
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    let stale_epoch = admission.capital_admission_nt_projection_epoch_for_test();
    let state_before_invalidation = admission
        .capital_admission_state_snapshot()
        .expect("test projection should publish state");

    admission.invalidate_capital_admission_for_nt_projection_request();
    let decision = admission.commit_capital_admission_nt_projection_for_test(
        stale_epoch,
        Some(fresh_components(2_000)),
        Some(2_000),
        BoltV3SubmitCapitalAdmissionOpenOrderSnapshot {
            observed_at_ns: 2_000,
            evidence_source: CapitalAdmissionRebuildSource::NtOpenOrderCache,
            observed_open_order_count: 0,
            all_open_orders_attributed: true,
            reservations: Vec::new(),
            live_non_reservation_client_order_ids: Default::default(),
        },
        2_000,
    );

    assert!(!decision.accepted, "a stale projection must not commit");
    assert_eq!(admission.capital_admission_reconciled(), Some(false));
    assert_eq!(
        admission.capital_admission_state_snapshot(),
        Some(state_before_invalidation),
        "a stale projection must perform zero state mutation"
    );
}

#[test]
fn only_one_nt_projection_candidate_can_commit_for_an_epoch() {
    let admission = capital_admission_configured_admission();
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    admission.invalidate_capital_admission_for_nt_projection_request();
    let shared_epoch = admission.capital_admission_nt_projection_epoch_for_test();
    let snapshot = BoltV3SubmitCapitalAdmissionOpenOrderSnapshot {
        observed_at_ns: 2_000,
        evidence_source: CapitalAdmissionRebuildSource::NtOpenOrderCache,
        observed_open_order_count: 0,
        all_open_orders_attributed: true,
        reservations: Vec::new(),
        live_non_reservation_client_order_ids: Default::default(),
    };

    let first = admission.commit_capital_admission_nt_projection_for_test(
        shared_epoch,
        Some(fresh_components(2_000)),
        Some(2_000),
        snapshot.clone(),
        2_000,
    );
    assert!(first.accepted, "the first candidate should commit");
    let state_after_first = admission
        .capital_admission_state_snapshot()
        .expect("the committed projection should publish state");

    let second = admission.commit_capital_admission_nt_projection_for_test(
        shared_epoch,
        Some(fresh_components(3_000)),
        Some(3_000),
        snapshot,
        3_000,
    );
    assert!(
        !second.accepted,
        "the consumed epoch must reject a competitor"
    );
    assert_eq!(
        admission.capital_admission_state_snapshot(),
        Some(state_after_first),
        "a rejected competing candidate must perform zero state mutation"
    );
}

#[test]
#[should_panic(expected = "capital admission runtime order-event feed lock poisoned")]
fn subscribed_order_event_panics_on_poisoned_capital_admission_feed_lock() {
    let feed = poisoned_capital_admission_runtime_feed();
    let _subscription = subscribe_submit_admission_nt_projection(Some(feed), no_op_nt_projection());

    publish_order_event(
        switchboard::get_event_order_topic(StrategyId::from("strategy-a")),
        &OrderEventAny::Canceled(order_canceled_event("client-order-1", 1_100)),
    );
}

#[test]
fn subscribed_position_event_requests_projection_without_locking_feed() {
    let feed = poisoned_capital_admission_runtime_feed();
    let projection_count = Rc::new(Cell::new(0));
    let count = Rc::clone(&projection_count);
    let _subscription = subscribe_submit_admission_nt_projection(
        Some(feed),
        Rc::new(move || count.set(count.get() + 1)),
    );

    publish_position_event(
        "events.position.ACCOUNT-001".into(),
        &adjusted_position_event(AccountId::from("ACCOUNT-001"), 1_100),
    );
    assert_eq!(projection_count.get(), 1);
}

#[test]
fn order_event_requests_submit_projection_without_capital_feed() {
    let projection_count = Rc::new(Cell::new(0));
    let count = Rc::clone(&projection_count);
    let _subscription =
        subscribe_submit_admission_nt_projection(None, Rc::new(move || count.set(count.get() + 1)));

    publish_order_event(
        switchboard::get_event_order_topic(StrategyId::from("strategy-a")),
        &OrderEventAny::Canceled(order_canceled_event("client-order-1", 1_100)),
    );

    assert_eq!(projection_count.get(), 1);
}

#[test]
fn subscribed_account_state_requests_projection_without_locking_feed() {
    let feed = poisoned_capital_admission_runtime_feed();
    let projection_count = Rc::new(Cell::new(0));
    let count = Rc::clone(&projection_count);
    let _subscription = subscribe_submit_admission_nt_projection(
        Some(feed),
        Rc::new(move || count.set(count.get() + 1)),
    );

    publish_account_state(
        "events.account.ACCOUNT-001".into(),
        &account_state(AccountId::from("ACCOUNT-001"), "USD", 1_100, 45.0),
    );
    assert_eq!(projection_count.get(), 1);
}

#[test]
fn subscribed_portfolio_snapshot_requests_projection_without_locking_feed() {
    let feed = poisoned_capital_admission_runtime_feed();
    let projection_count = Rc::new(Cell::new(0));
    let count = Rc::clone(&projection_count);
    let _subscription = subscribe_submit_admission_nt_projection(
        Some(feed),
        Rc::new(move || count.set(count.get() + 1)),
    );

    publish_portfolio_snapshot(
        "events.portfolio.ACCOUNT-001".into(),
        &portfolio_snapshot(AccountId::from("ACCOUNT-001"), "USD", 1_100, 45.0),
    );
    assert_eq!(projection_count.get(), 1);
}

#[test]
fn subscribed_account_and_portfolio_events_remain_advisory_without_provider_collateral_allowance() {
    let admission = Arc::new(capital_admission_configured_admission());
    let feed = Arc::new(Mutex::new(CapitalAdmissionRuntimeFeed::new(
        runtime_feed_config(),
        admission.clone(),
    )));
    let mut subscription =
        subscribe_submit_admission_nt_projection(Some(feed), no_op_nt_projection());

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
fn polymarket_provider_collateral_allowance_snapshot_alone_cannot_publish_money_readiness() {
    let admission = Arc::new(polymarket_capital_admission_configured_admission());
    let mut feed =
        CapitalAdmissionRuntimeFeed::new(polymarket_runtime_feed_config(), admission.clone());

    feed.on_provider_collateral_allowance_snapshot(
        polymarket_provider_collateral_allowance_snapshot(
            1_200,
            Decimal::new(45_000_000, 0),
            Decimal::new(40_000_000, 0),
        ),
    );
    assert_eq!(feed.accepted_allowance_observed_at_ns(), Some(1_200));
    assert_eq!(admission.capital_admission_state_snapshot(), None);
}

#[test]
fn provider_collateral_allowance_combines_with_nt_owned_order_and_position_state() {
    let admission = Arc::new(polymarket_capital_admission_configured_admission());
    let mut feed =
        CapitalAdmissionRuntimeFeed::new(polymarket_runtime_feed_config(), admission.clone());

    feed.on_provider_collateral_allowance_snapshot(
        polymarket_provider_collateral_allowance_snapshot(
            1_200,
            Decimal::new(45_000_000, 0),
            Decimal::new(40_000_000, 0),
        ),
    );

    let components = feed
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: Some(1_200),
            account_balances: Some((Decimal::new(45, 0), Decimal::new(45, 0))),
            open_client_order_ids: vec!["client-order-1".to_string()],
            yes_position: Decimal::new(99, 0),
            no_position: Decimal::new(88, 0),
            observed_at_ns: 1_300,
        })
        .expect("fresh provider collateral allowance and canonical NT state should combine");

    assert_eq!(components.order_lifecycle.source, "nt_open_order_cache");
    assert_eq!(components.order_lifecycle.open_order_count, 1);
    assert!(
        !components.order_lifecycle.all_open_orders_attributed,
        "Bolt evidence recovery, not provider data, must attribute NT open orders"
    );

    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(product.source, "nt_position_cache");
    assert_eq!(product.observed_at_ns, 1_300);
    assert_eq!(product.yes_position, Decimal::new(99, 0));
    assert_eq!(product.no_position, Decimal::new(88, 0));
    assert_eq!(product.collateral_allowance, Decimal::new(40, 0));
}

#[test]
fn canonical_nt_projection_rejects_incomplete_stale_or_duplicate_inputs() {
    let admission = Arc::new(polymarket_capital_admission_configured_admission());
    let mut feed =
        CapitalAdmissionRuntimeFeed::new(polymarket_runtime_feed_config(), admission.clone());
    let projection = CapitalAdmissionNtCacheProjection {
        accepted_allowance_observed_at_ns: feed.accepted_allowance_observed_at_ns(),
        account_balances: Some((Decimal::new(45, 0), Decimal::new(45, 0))),
        open_client_order_ids: Vec::new(),
        yes_position: Decimal::ZERO,
        no_position: Decimal::ZERO,
        observed_at_ns: 1_300,
    };

    assert_eq!(
        feed.canonical_nt_components(projection.clone()),
        Err(CapitalAdmissionProjectionError::MissingProviderCollateralAllowance)
    );

    feed.on_provider_collateral_allowance_snapshot(
        polymarket_provider_collateral_allowance_snapshot(
            1_200,
            Decimal::new(45_000_000, 0),
            Decimal::new(40_000_000, 0),
        ),
    );
    let mut current = projection;
    current.accepted_allowance_observed_at_ns = Some(1_200);

    let mut missing_balance = current.clone();
    missing_balance.account_balances = None;
    assert_eq!(
        feed.canonical_nt_components(missing_balance),
        Err(CapitalAdmissionProjectionError::MissingNtAccountBalances)
    );

    let mut stale_generation = current.clone();
    stale_generation.accepted_allowance_observed_at_ns = Some(1_100);
    assert_eq!(
        feed.canonical_nt_components(stale_generation),
        Err(
            CapitalAdmissionProjectionError::AllowanceGenerationMismatch {
                accepted: Some(1_200),
                projected: Some(1_100),
            }
        )
    );

    current.open_client_order_ids =
        vec!["client-order-1".to_string(), "client-order-1".to_string()];
    assert_eq!(
        feed.canonical_nt_components(current),
        Err(CapitalAdmissionProjectionError::DuplicateNtClientOrderId)
    );
}

#[test]
fn canonical_nt_projection_rejects_stale_callback_attribution() {
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

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_200, 100,
    ));
    let components = feed
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: Some(1_200),
            account_balances: Some((Decimal::new(100, 0), Decimal::new(100, 0))),
            open_client_order_ids: Vec::new(),
            yes_position: Decimal::ZERO,
            no_position: Decimal::ZERO,
            observed_at_ns: 1_250,
        })
        .expect("canonical empty NT projection should be complete");

    assert_eq!(components.order_lifecycle.source, "nt_open_order_cache");
    assert_eq!(
        components.order_lifecycle.open_order_count, 0,
        "canonical NT open-order count must not be overwritten by stale callback memory"
    );
    assert!(
        components.order_lifecycle.all_open_orders_attributed,
        "raw order callbacks must not survive into the canonical empty NT projection"
    );
}

#[test]
fn provider_collateral_allowance_survives_nt_cache_projection_and_reservation_rebuild() {
    let admission = Arc::new(polymarket_capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed =
        CapitalAdmissionRuntimeFeed::new(polymarket_runtime_feed_config(), admission.clone());

    feed.on_provider_collateral_allowance_snapshot(
        polymarket_provider_collateral_allowance_snapshot(
            1_200,
            Decimal::new(100_000_000, 0),
            Decimal::new(100_000_000, 0),
        ),
    );
    let projection = CapitalAdmissionNtCacheProjection {
        accepted_allowance_observed_at_ns: Some(1_200),
        account_balances: Some((Decimal::new(100, 0), Decimal::new(100, 0))),
        open_client_order_ids: vec!["client-order-1".to_string()],
        yes_position: Decimal::new(99, 0),
        no_position: Decimal::new(88, 0),
        observed_at_ns: 1_300,
    };
    let components = feed
        .canonical_nt_components(projection.clone())
        .expect("canonical NT projection should be complete before reservation rebuild");
    admission.update_capital_admission_nt_components(components);

    let mut recovered_reservation = open_order_reservation(
        "client-order-1",
        "client-order-1#rebuilt",
        Decimal::new(43, 1),
    );
    recovered_reservation.observed_at_ns = 1_350;
    let rebuild = admission.rebuild_capital_admission_open_order_reservations_for_test(
        vec![recovered_reservation],
        1_350,
    );
    assert!(rebuild.accepted, "rebuild should accept: {rebuild:?}");
    let components = feed
        .canonical_nt_components(projection)
        .expect("canonical NT projection should remain complete after reservation rebuild");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(product.source, "nt_position_cache");
    assert_eq!(product.yes_position, Decimal::new(99, 0));
    assert_eq!(product.no_position, Decimal::new(88, 0));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("accepted rebuild should publish capital admission state");
    assert_eq!(
        state.order_lifecycle.source,
        "bolt_recovered_open_order_reservations"
    );
    assert_eq!(state.order_lifecycle.open_order_count, 1);
    assert!(
        state.order_lifecycle.all_open_orders_attributed,
        "only the evidence-backed reservation rebuild may mark NT orders attributed"
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}

#[test]
fn provider_collateral_allowance_snapshot_cannot_replace_nt_portfolio_or_position_state() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    seed_provider_collateral_allowance(&mut feed, 1_000);
    let components = feed
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: feed.accepted_allowance_observed_at_ns(),
            account_balances: Some((Decimal::new(88, 0), Decimal::new(99, 0))),
            open_client_order_ids: Vec::new(),
            yes_position: Decimal::new(11, 0),
            no_position: Decimal::new(3, 0),
            observed_at_ns: 1_150,
        })
        .expect("canonical NT projection should combine with provider collateral allowance");
    admission.update_capital_admission_nt_components(components);

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_200, 40,
    ));

    let state = admission
        .capital_admission_state_snapshot()
        .expect("canonical NT state should remain published");
    assert_eq!(state.portfolio.source, "nt_account_cache");
    assert_eq!(state.portfolio.free_collateral, Decimal::new(88, 0));
    assert_eq!(state.portfolio.total_equity, Decimal::new(99, 0));
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.source, "nt_position_cache");
    assert_eq!(product.yes_position, Decimal::new(11, 0));
    assert_eq!(product.no_position, Decimal::new(3, 0));
}

#[test]
fn polymarket_allowance_is_not_min_clamped_by_nt_account_free_collateral() {
    let admission = Arc::new(polymarket_capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(polymarket_runtime_feed_config(), admission);

    feed.on_provider_collateral_allowance_snapshot(
        polymarket_provider_collateral_allowance_snapshot(
            1_200,
            Decimal::new(45_000_000, 0),
            Decimal::new(40_000_000, 0),
        ),
    );
    let components = feed
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: Some(1_200),
            account_balances: Some((Decimal::new(10, 0), Decimal::new(10, 0))),
            open_client_order_ids: Vec::new(),
            yes_position: Decimal::ZERO,
            no_position: Decimal::ZERO,
            observed_at_ns: 1_250,
        })
        .expect("canonical NT state should combine with provider-only allowance");

    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(
        product.collateral_allowance,
        Decimal::new(40, 0),
        "provider allowance must not be min-clamped by NT free collateral"
    );
}

#[test]
fn provider_worker_cannot_reopen_admission_without_nt_projection() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::new();
    let admission = Arc::new(capital_admission_configured_admission_with_writer(
        writer.recorder(),
    ));
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.project_account_fixture(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    let _ = feed.project_portfolio_fixture(&portfolio_snapshot(
        AccountId::from("ACCOUNT-001"),
        "USD",
        950,
        100.0,
    ));
    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_000, 100,
    ));
    apply_empty_canonical_nt_projection(&mut feed, &admission, 1_025);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_050)
        .expect("fresh sizing state should admit before degraded venue authority")
        .commit_submitted();

    admission.suspend_capital_admission_for_provider_collateral_allowance_capture_failure(
        ProviderCollateralAllowanceCaptureFailureEvidence {
            source: POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE.to_string(),
            observed_at_ns: 1_100,
            endpoint: ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance,
            error_class: ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode,
            captures_missed: 1,
        },
    );

    assert_eq!(
        admission.kill_switch_state_kind(),
        KillSwitchStateKind::Armed
    );
    assert_eq!(admission.capital_admission_reconciled(), Some(false));
    let capture_failures = writer.provider_collateral_allowance_capture_failures();
    assert_eq!(capture_failures.len(), 1);
    assert_eq!(
        capture_failures[0].endpoint,
        EvidenceCaptureEndpoint::ClobBalanceAllowance
    );
    assert_eq!(
        capture_failures[0].error_class,
        EvidenceCaptureErrorClass::TransportOrDecode
    );
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

    let _ = feed.project_account_fixture(&account_state(
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
    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_200, 100,
    ));

    assert_eq!(
        admission.capital_admission_reconciled(),
        Some(false),
        "provider-thread success may update readiness input but only an NT-backed projection may reopen admission"
    );
}

#[test]
fn accepted_allowance_at_failure_watermark_does_not_clear_capture_failure_suspension() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_000, 100,
    ));
    apply_empty_canonical_nt_projection(&mut feed, &admission, 1_025);

    admission.suspend_capital_admission_for_provider_collateral_allowance_capture_failure(
        ProviderCollateralAllowanceCaptureFailureEvidence {
            source: POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE.to_string(),
            observed_at_ns: 1_100,
            endpoint: ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance,
            error_class: ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode,
            captures_missed: 1,
        },
    );
    assert_eq!(admission.capital_admission_reconciled(), Some(false));

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_100, 100,
    ));
    apply_empty_canonical_nt_projection(&mut feed, &admission, 1_100);
    assert_eq!(
        admission.capital_admission_reconciled(),
        Some(false),
        "accepted allowance at the failure watermark must not clear suspension"
    );

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_101, 100,
    ));
    apply_empty_canonical_nt_projection(&mut feed, &admission, 1_101);
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
    let mut subscription =
        subscribe_submit_admission_nt_projection(Some(feed.clone()), no_op_nt_projection());
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
}

#[test]
fn feed_waits_for_matching_account_identity_before_publish() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_000, 40,
    ));
    assert_eq!(admission.capital_admission_state_snapshot(), None);
    let components = feed
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: Some(1_000),
            account_balances: Some((Decimal::new(45, 0), Decimal::new(50, 0))),
            open_client_order_ids: Vec::new(),
            yes_position: Decimal::ZERO,
            no_position: Decimal::ZERO,
            observed_at_ns: 1_010,
        })
        .expect("matching NT projection should publish after provider attestation");
    admission.update_capital_admission_nt_components(components);
    assert!(admission.capital_admission_state_snapshot().is_some());

    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let mut wrong_account_allowance = provider_collateral_allowance_snapshot(1_025, 40);
    wrong_account_allowance.account_id = "OTHER-ACCOUNT".to_string();
    feed.on_provider_collateral_allowance_snapshot(wrong_account_allowance);
    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_050, 40,
    ));
    assert_eq!(admission.capital_admission_state_snapshot(), None);

    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    assert!(
        feed.project_portfolio_fixture(&portfolio_snapshot(
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
        feed.project_account_fixture(&account_state(
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
        feed.project_account_fixture(&account_state(
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
        feed.project_portfolio_fixture(&portfolio_snapshot(
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
fn feed_does_not_derive_default_provider_collateral_allowance_from_nt_account_free_collateral() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.project_account_fixture(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_000,
            45.0,
        ))
        .is_none(),
        "NT AccountState is advisory-only and must not create provider collateral allowance"
    );
    assert_eq!(admission.capital_admission_state_snapshot(), None);
}

#[test]
fn feed_derives_collateral_allowance_from_venue_allowance() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(950, 25));
    let components = feed
        .project_account_fixture(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_000,
            100.0,
        ))
        .expect("fixture projection should include provider collateral allowance");

    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(product.collateral_allowance, Decimal::new(25, 0));
    assert_eq!(
        components
            .provider_collateral_allowance
            .collateral_allowance,
        Decimal::new(25, 0)
    );
}

#[test]
fn new_provider_collateral_allowance_snapshot_revokes_admission_until_nt_reprojection() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_000, 100,
    ));
    apply_empty_canonical_nt_projection(&mut feed, &admission, 1_025);
    assert_eq!(admission.capital_admission_reconciled(), Some(true));

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_100, 100,
    ));

    assert_eq!(admission.capital_admission_reconciled(), Some(false));
    assert!(matches!(
        admission.admit_at(&capital_admission_submit_request("client-order-1"), 1_101),
        Err(BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
            reason: BoltV3CapitalAdmissionRejectReason::ReconciliationRequired
        })
    ));
}

#[test]
fn account_update_does_not_make_external_provider_collateral_allowance_fresh() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission);

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(100, 25));
    let components = feed
        .project_account_fixture(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            10_000,
            100.0,
        ))
        .expect("account and venue state should publish");

    assert_eq!(
        components.provider_collateral_allowance.observed_at_ns, 100,
        "fresh NT account state must not refresh externally sourced venue evidence"
    );
}

#[test]
fn recomputed_product_allowance_carries_fresh_component_timestamp() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut config = runtime_feed_config();
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = &mut config.product_state;
    product.observed_at_ns = 0;
    let mut feed = CapitalAdmissionRuntimeFeed::new(config, admission);

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        10_000, 25,
    ));
    let components = feed
        .project_account_fixture(&account_state(
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
fn feed_ignores_provider_allowance_identity_mismatch_until_matching_snapshot_arrives() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        900, 100,
    ));
    let components = feed
        .project_account_fixture(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .expect("canonical NT projection should combine with provider input");
    admission.update_capital_admission_nt_components(components);

    let mut mismatched = provider_collateral_allowance_snapshot(1_100, 100);
    mismatched.venue_id = "VENUE-B".to_string();
    feed.on_provider_collateral_allowance_snapshot(mismatched);
    let state = admission
        .capital_admission_state_snapshot()
        .expect("mismatched allowance must not clear the last valid state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.collateral_allowance, Decimal::new(100, 0));
    assert!(
        feed.project_portfolio_fixture(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_200,
            100.0
        ))
        .is_some()
    );

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_300, 50,
    ));
    let components = feed
        .project_account_fixture(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_350,
            100.0,
        ))
        .expect("next canonical NT projection should combine the accepted provider input");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = components.product_state;
    assert_eq!(product.collateral_allowance, Decimal::new(50, 0));
}

#[test]
fn feed_ignores_older_allowance_snapshot_after_newer_one() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        1_000, 40,
    ));
    let components = feed
        .project_account_fixture(&account_state(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_050,
            100.0,
        ))
        .expect("canonical NT projection should combine with provider input");
    admission.update_capital_admission_nt_components(components);

    feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(900, 5));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("older allowance should not clear or regress the latest snapshot");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.collateral_allowance, Decimal::new(40, 0));
}

#[test]
fn feed_ignores_account_state_for_other_collateral_currency() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    assert!(
        feed.project_portfolio_fixture(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_000,
            50.0
        ))
        .is_none()
    );
    assert!(
        feed.project_account_fixture(&account_state(
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
fn capital_admission_rebuild_evidence_failure_leaves_gate_unreconciled() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = capital_admission_configured_admission_with_writer(writer.recorder());
    admission.update_capital_admission_nt_components(fresh_components(900));
    writer.fail_purpose_on_attempt(
        bolt_v2::bolt_v3_current_evidence::CurrentEvidenceTestPurpose::CapitalAdmissionRebuild,
        1,
    );

    let rebuild =
        admission.rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), 1_000);

    assert!(!rebuild.accepted);
    assert_eq!(
        rebuild.reason,
        Some(ReservationRejectionReason::MissingEvidence)
    );
    assert_eq!(admission.capital_admission_reconciled(), Some(false));
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn terminal_callback_cannot_reopen_gate_without_fresh_nt_projection() {
    let admission = Arc::new(polymarket_capital_admission_configured_admission());
    let mut feed =
        CapitalAdmissionRuntimeFeed::new(polymarket_runtime_feed_config(), admission.clone());

    feed.on_provider_collateral_allowance_snapshot(
        polymarket_provider_collateral_allowance_snapshot(
            1_200,
            Decimal::new(45_000_000, 0),
            Decimal::new(40_000_000, 0),
        ),
    );
    let components = feed
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: Some(1_200),
            account_balances: Some((Decimal::new(100, 0), Decimal::new(100, 0))),
            open_client_order_ids: vec!["client-order-1".to_string()],
            yes_position: Decimal::ZERO,
            no_position: Decimal::ZERO,
            observed_at_ns: 1_250,
        })
        .expect("canonical projection should expose the unattributed NT order");
    admission.update_capital_admission_nt_components(components);
    assert_eq!(admission.capital_admission_reconciled(), Some(false));

    let _ = feed.on_order_event(&OrderEventAny::Canceled(order_canceled_event(
        "client-order-1",
        1_300,
    )));

    assert_eq!(admission.capital_admission_reconciled(), Some(false));
    let state = admission
        .capital_admission_state_snapshot()
        .expect("terminal callback should preserve the last canonical NT projection");
    assert_eq!(state.order_lifecycle.open_order_count, 1);
    assert!(!state.order_lifecycle.all_open_orders_attributed);
    assert_eq!(
        admission
            .admit_at(&capital_admission_submit_request("client-order-1"), 1_350)
            .expect_err("only a fresh canonical NT projection may reopen admission"),
        BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
            reason: BoltV3CapitalAdmissionRejectReason::VenueMismatch,
        }
    );
}

#[test]
fn capital_admission_cache_seed_updates_configured_yes_no_inventory() {
    let admission = Arc::new(capital_admission_configured_admission());
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());

    let _ = feed.project_account_fixture(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        1_000,
        45.0,
    ));
    seed_provider_collateral_allowance(&mut feed, 1_050);
    assert!(
        feed.project_portfolio_fixture(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            1_100,
            50.0
        ))
        .is_some()
    );
    let components = feed
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: feed.accepted_allowance_observed_at_ns(),
            account_balances: Some((Decimal::new(50, 0), Decimal::new(50, 0))),
            open_client_order_ids: Vec::new(),
            yes_position: Decimal::new(7, 0),
            no_position: Decimal::new(2, 0),
            observed_at_ns: 1_200,
        })
        .expect("canonical NT projection should publish configured product inventory");
    admission.update_capital_admission_nt_components(components);

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
fn partial_fill_event_records_evidence_without_revaluing_nt_derived_reservation() {
    let (admission, mut feed) = committed_submit_runtime_feed();
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
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("matching fill should record Bolt audit evidence");

    assert!(decision.accepted);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("canonical NT projection should remain available");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.source, "nt_position_cache");
    assert_eq!(product.observed_at_ns, 1_000);
    assert_eq!(product.yes_position, Decimal::ZERO);
}

#[test]
fn unknown_raw_fill_preserves_nt_position_and_latches_admission_fail_closed() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.project_account_fixture(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_provider_collateral_allowance(&mut feed, 925);
    assert!(
        feed.project_portfolio_fixture(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    let components = feed
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: feed.accepted_allowance_observed_at_ns(),
            account_balances: Some((Decimal::new(100, 0), Decimal::new(100, 0))),
            open_client_order_ids: Vec::new(),
            yes_position: Decimal::new(3, 0),
            no_position: Decimal::ZERO,
            observed_at_ns: 1_000,
        })
        .expect("canonical NT projection should publish seeded product state");
    admission.update_capital_admission_nt_components(components);
    rebuild_empty_capital_admission(&admission);

    let decision = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "external-order-1",
            "external-trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(3),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("relevant unknown fill must produce a fail-closed decision");
    assert!(!decision.accepted);
    assert!(decision.unknown_reservation);
    assert_eq!(admission.capital_admission_reconciled(), Some(false));

    let state = admission
        .capital_admission_state_snapshot()
        .expect("raw fill callback should preserve canonical NT product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_eq!(product.source, "nt_position_cache");
    assert_eq!(product.observed_at_ns, 1_000);
    assert_eq!(product.yes_position, Decimal::new(3, 0));
}

#[test]
fn full_fill_event_cannot_release_reservation_before_nt_reprojection() {
    let (admission, mut feed) = committed_submit_runtime_feed();
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
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("matching full fill should record Bolt audit evidence");

    assert!(decision.accepted);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("canonical NT lifecycle should remain available");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
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
fn fill_event_for_rebuilt_reservation_records_evidence_without_revaluation() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let mut components = fresh_components(1_000);
    components.order_lifecycle.open_order_count = 1;
    components.order_lifecycle.all_open_orders_attributed = true;
    admission.update_capital_admission_nt_components(components);
    let rebuild = admission.rebuild_capital_admission_open_order_reservations_for_test(
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
        .expect("rebuilt reservation metadata should support fill audit evidence");
    assert!(decision.accepted);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}

#[test]
fn reconciliation_fill_for_recovered_startup_reservation_is_idempotent() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::new();
    let admission = Arc::new(capital_admission_configured_admission_with_writer(
        writer.recorder(),
    ));
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let mut components = fresh_components(1_000);
    components.order_lifecycle.open_order_count = 1;
    components.order_lifecycle.all_open_orders_attributed = true;
    admission.update_capital_admission_nt_components(components);
    let reservation = open_order_reservation(
        "client-order-1",
        "client-order-1#rebuilt",
        Decimal::new(43, 1),
    );
    let rebuild = admission
        .rebuild_capital_admission_open_order_reservations_for_test(vec![reservation], 1_000);
    assert!(rebuild.accepted);

    let unseen_reconciliation = feed
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
        .expect("unseen startup reconciliation fill must be durably recorded");
    assert!(unseen_reconciliation.accepted);
    assert_eq!(
        writer
            .facts()
            .into_iter()
            .filter(|fact| matches!(
                fact,
                bolt_v2::bolt_v3_current_evidence::CurrentFact::SubmitReservationFill(_)
            ))
            .count(),
        1
    );
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
    assert_eq!(
        writer
            .facts()
            .into_iter()
            .filter(|fact| matches!(
                fact,
                bolt_v2::bolt_v3_current_evidence::CurrentFact::SubmitReservationFill(_)
            ))
            .count(),
        1,
        "the duplicate reconciliation fill must not append"
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    assert!(
        feed.on_order_event(&OrderEventAny::Canceled(order_canceled_event(
            "client-order-1",
            1_300,
        )))
        .is_none()
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let _duplicate_after_terminal = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_400,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("raw terminal callback cannot delete the NT-derived reservation");
    let state = admission
        .capital_admission_state_snapshot()
        .expect("post-terminal duplicate reconciliation fill should not mutate product state");
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    assert_ne!(product.source, "nt_order_fill");
    assert_eq!(product.yes_position, Decimal::new(10, 0));
}

#[test]
fn attributed_rebuild_after_cache_seed_keeps_next_submit_open() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.project_account_fixture(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_provider_collateral_allowance(&mut feed, 925);
    assert!(
        feed.project_portfolio_fixture(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    let components = feed
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: feed.accepted_allowance_observed_at_ns(),
            account_balances: Some((Decimal::new(100, 0), Decimal::new(100, 0))),
            open_client_order_ids: vec!["client-order-1".to_string()],
            yes_position: Decimal::ZERO,
            no_position: Decimal::ZERO,
            observed_at_ns: 1_000,
        })
        .expect("canonical NT projection should retain the startup order");
    admission.update_capital_admission_nt_components(components);

    let rebuild = admission.rebuild_capital_admission_open_order_reservations_for_test(
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
fn delayed_duplicate_fill_after_empty_nt_reprojection_is_idempotent() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    let fill = OrderEventAny::Filled(order_filled_event_with(
        "client-order-1",
        "trade-1",
        1_100,
        AccountId::from("ACCOUNT-001"),
        Quantity::from(4),
        OrderSide::Buy,
        InstrumentId::from("instrument-yes.VENUE-A"),
    ));
    let first = feed
        .on_order_event(&fill)
        .expect("the first attributed fill should be recorded");
    assert!(first.accepted);

    apply_empty_canonical_nt_projection(&mut feed, &admission, 1_200);
    let duplicate = feed
        .on_order_event(&fill)
        .expect("the committed fill must remain idempotent after reprojection");

    assert!(duplicate.accepted);
    assert!(!duplicate.unknown_reservation);
    assert_eq!(admission.capital_admission_reconciled(), Some(true));
}

#[test]
fn delayed_duplicate_fill_with_conflicting_quantity_after_empty_nt_reprojection_fails_closed() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    let first = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("the first attributed fill should be recorded");
    assert!(first.accepted);

    apply_empty_canonical_nt_projection(&mut feed, &admission, 1_200);
    let conflicting = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_300,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(5),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("a conflicting durable fill identity must produce a fail-closed decision");

    assert!(!conflicting.accepted);
    assert!(conflicting.unknown_reservation);
    assert_eq!(admission.capital_admission_reconciled(), Some(false));
}

#[test]
fn fill_evidence_invalidates_an_in_flight_nt_projection_candidate() {
    let (admission, mut feed) = committed_submit_runtime_feed();
    let stale_epoch = admission.capital_admission_nt_projection_epoch_for_test();
    let fill = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("the attributed fill should be recorded");
    assert!(fill.accepted);

    let stale = admission.commit_capital_admission_nt_projection_for_test(
        stale_epoch,
        Some(fresh_components(1_200)),
        Some(1_200),
        BoltV3SubmitCapitalAdmissionOpenOrderSnapshot {
            observed_at_ns: 1_200,
            evidence_source: CapitalAdmissionRebuildSource::NtOpenOrderCache,
            observed_open_order_count: 0,
            all_open_orders_attributed: true,
            reservations: Vec::new(),
            live_non_reservation_client_order_ids: Default::default(),
        },
        1_200,
    );

    assert!(!stale.accepted);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1)),
        "a projection captured before the fill must not erase its reservation"
    );
}

#[test]
fn full_fill_event_for_rebuilt_reservation_waits_for_nt_reprojection() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    let _ = feed.project_account_fixture(&account_state(
        AccountId::from("ACCOUNT-001"),
        "USD",
        900,
        100.0,
    ));
    seed_provider_collateral_allowance(&mut feed, 925);
    assert!(
        feed.project_portfolio_fixture(&portfolio_snapshot(
            AccountId::from("ACCOUNT-001"),
            "USD",
            950,
            100.0,
        ))
        .is_some()
    );
    let components = feed
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: feed.accepted_allowance_observed_at_ns(),
            account_balances: Some((Decimal::new(100, 0), Decimal::new(100, 0))),
            open_client_order_ids: vec!["client-order-1".to_string()],
            yes_position: Decimal::ZERO,
            no_position: Decimal::ZERO,
            observed_at_ns: 1_000,
        })
        .expect("canonical NT projection should retain the startup order");
    admission.update_capital_admission_nt_components(components);
    let rebuild = admission.rebuild_capital_admission_open_order_reservations_for_test(
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
        .expect("rebuilt reservation full fill should record audit evidence");

    assert!(decision.accepted);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("canonical NT lifecycle should remain available");
    assert_eq!(state.order_lifecycle.open_order_count, 1);
    assert!(state.order_lifecycle.all_open_orders_attributed);
}

#[test]
fn duplicate_trade_id_with_conflicting_runtime_instrument_latches_fail_closed() {
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
    let decision = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "client-order-1",
            "trade-1",
            1_200,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(4),
            OrderSide::Buy,
            InstrumentId::from("instrument-no.VENUE-A"),
        )))
        .expect("conflicting duplicate fill must produce a fail-closed decision");
    assert!(!decision.accepted);
    assert!(decision.unknown_reservation);
    assert_eq!(admission.capital_admission_reconciled(), Some(false));
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn terminal_event_after_partial_fill_cannot_release_without_nt_reprojection() {
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

    assert!(
        feed.on_order_event(&OrderEventAny::Canceled(order_canceled_event(
            "client-order-1",
            1_200,
        )))
        .is_none()
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    let state = admission
        .capital_admission_state_snapshot()
        .expect("terminal should publish lifecycle");
    assert_eq!(state.order_lifecycle.open_order_count, 0);
}

#[test]
fn terminal_nt_order_event_cannot_release_committed_reservation_without_reprojection() {
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
    assert!(
        feed.on_order_event(&OrderEventAny::Canceled(order_canceled_event(
            "client-order-1",
            1_100,
        )))
        .is_none()
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}

#[test]
fn admission_evidence_failure_rolls_back_capital_reservation_before_submit() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = capital_admission_configured_admission_with_writer(writer.recorder());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    writer.fail_purpose_on_attempt(
        bolt_v2::bolt_v3_current_evidence::CurrentEvidenceTestPurpose::AdmittedEntryAdmission,
        1,
    );

    let error = admission
        .admit_at(
            &capital_admission_submit_request("failed-evidence-order"),
            1_000,
        )
        .expect_err("machine-evidence failure must reject before provider submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::EvidenceWriteFailed { .. }
    ));
    assert_eq!(admission.admitted_order_count(), 0);
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO),
        "the pre-submit capital reservation must be rolled back"
    );
    assert!(
        !admission.capital_admission_has_live_reservation("failed-evidence-order"),
        "no reservation may survive a rejected evidence boundary"
    );
    assert_eq!(
        writer.reservation_attributions().len(),
        0,
        "atomic reservation attribution must not survive a failed admission append"
    );
}

#[test]
fn configured_submit_sizer_rejects_stale_provider_collateral_allowance_before_nt_submit() {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut components = fresh_components(900);
    components.provider_collateral_allowance.observed_at_ns = 100;
    admission.update_capital_admission_nt_components(components);
    rebuild_empty_capital_admission(&admission);

    let error = admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect_err("stale provider collateral allowance evidence must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
            reason: BoltV3CapitalAdmissionRejectReason::StaleNtState
        }
    ));
}

#[test]
fn subscribed_terminal_nt_order_event_only_requests_nt_reprojection() {
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
    let mut subscription =
        subscribe_submit_admission_nt_projection(Some(feed.clone()), no_op_nt_projection());

    publish_order_event(
        switchboard::get_event_order_topic(StrategyId::from("strategy-a")),
        &OrderEventAny::Canceled(order_canceled_event("client-order-1", 1_100)),
    );
    subscription.unsubscribe_all();

    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}

#[test]
fn denied_nt_order_event_without_account_cannot_release_reservation() {
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
        feed.on_order_event(&OrderEventAny::Denied(order_denied_event(
            "client-order-1",
            1_100,
        )))
        .is_none()
    );
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}

#[test]
fn rejected_and_expired_nt_order_events_cannot_release_without_reprojection() {
    assert_terminal_event_does_not_release(
        "client-order-rejected",
        OrderEventAny::Rejected(order_rejected_event(
            "client-order-rejected",
            1_100,
            AccountId::from("ACCOUNT-001"),
        )),
    );
    assert_terminal_event_does_not_release(
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
}

#[test]
fn unknown_relevant_fill_latches_capital_admission_fail_closed() {
    let (admission, mut feed) = committed_submit_runtime_feed();

    let decision = feed
        .on_order_event(&OrderEventAny::Filled(order_filled_event_with(
            "unknown-client-order",
            "unknown-trade",
            1_100,
            AccountId::from("ACCOUNT-001"),
            Quantity::from(1),
            OrderSide::Buy,
            InstrumentId::from("instrument-yes.VENUE-A"),
        )))
        .expect("a relevant unknown fill must return a fail-closed decision");

    assert!(!decision.accepted);
    assert!(decision.unknown_reservation);
    assert_eq!(admission.capital_admission_reconciled(), Some(false));

    admission.update_capital_admission_nt_components(fresh_components(1_200));
    let rebuild =
        admission.rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), 1_200);
    assert!(!rebuild.accepted);
    assert_eq!(admission.capital_admission_reconciled(), Some(false));
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
}

fn assert_terminal_event_does_not_release(client_order_id: &str, event: OrderEventAny) {
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    admission.update_capital_admission_nt_components(fresh_components(900));
    rebuild_empty_capital_admission(&admission);
    admission
        .admit_at(&capital_admission_submit_request(client_order_id), 1_000)
        .expect("fresh sizing state should admit")
        .commit_submitted();

    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    assert!(feed.on_order_event(&event).is_none());
    assert_eq!(
        admission.capital_admission_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
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
                collateral_coupled_group_id: "group-1".to_string(),
            },
        ),
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
        None,
        false,
        vec![],
        vec![],
        vec![],
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
    capital_admission_configured_admission_with_writer(
        support::current_evidence::recording_evidence(),
    )
}

fn capital_admission_configured_admission_with_writer(
    writer: Arc<DecisionEvidenceRecorder>,
) -> BoltV3SubmitAdmissionState {
    capital_admission_configured_admission_with_writer_and_venue(writer, "VENUE-A")
}

fn polymarket_capital_admission_configured_admission() -> BoltV3SubmitAdmissionState {
    capital_admission_configured_admission_with_writer_and_venue(
        support::current_evidence::recording_evidence(),
        "POLYMARKET",
    )
}

fn capital_admission_configured_admission_with_writer_and_venue(
    writer: Arc<DecisionEvidenceRecorder>,
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
                fee_slippage_policy: Some(FeeSlippagePolicy {
                    max_fee_liability: Decimal::new(10, 2),
                    max_slippage_liability: Decimal::new(20, 2),
                }),
            },
        },
    )
}

fn arm_default(_admission: &BoltV3SubmitAdmissionState) {}

fn rebuild_empty_capital_admission(admission: &BoltV3SubmitAdmissionState) {
    let rebuild =
        admission.rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), 1_000);
    assert!(
        rebuild.accepted,
        "test startup rebuild should open submit admission"
    );
    assert_eq!(admission.capital_admission_reconciled(), Some(true));
}

fn apply_empty_canonical_nt_projection(
    feed: &mut CapitalAdmissionRuntimeFeed,
    admission: &BoltV3SubmitAdmissionState,
    observed_at_ns: u64,
) {
    let accepted_allowance_observed_at_ns = feed.accepted_allowance_observed_at_ns();
    let projection = CapitalAdmissionNtCacheProjection {
        accepted_allowance_observed_at_ns,
        account_balances: Some((Decimal::new(100, 0), Decimal::new(100, 0))),
        open_client_order_ids: Vec::new(),
        yes_position: Decimal::ZERO,
        no_position: Decimal::ZERO,
        observed_at_ns,
    };
    let components = feed
        .canonical_nt_components(projection.clone())
        .expect("canonical NT projection should be complete");
    admission.update_capital_admission_nt_components(components);
    let rebuild = admission
        .rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), observed_at_ns);
    assert!(
        rebuild.accepted,
        "canonical empty NT projection should rebuild the reservation ledger"
    );
    let components = feed
        .canonical_nt_components(projection)
        .expect("rebuilt canonical NT projection should be complete");
    admission.update_capital_admission_nt_components_after_accepted_allowance_snapshot(
        components,
        accepted_allowance_observed_at_ns
            .expect("provider collateral allowance should precede canonical projection"),
    );
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
        observed_at_ns: 1_000,
        evidence_label: "nt_open_order_cache".to_string(),
    }
}

fn committed_submit_runtime_feed() -> (Arc<BoltV3SubmitAdmissionState>, CapitalAdmissionRuntimeFeed)
{
    let admission = Arc::new(capital_admission_configured_admission());
    arm_default(&admission);
    let mut feed = CapitalAdmissionRuntimeFeed::new(runtime_feed_config(), admission.clone());
    seed_provider_collateral_allowance(&mut feed, 925);
    apply_empty_canonical_nt_projection(&mut feed, &admission, 1_000);
    admission
        .admit_at(&capital_admission_submit_request("client-order-1"), 1_000)
        .expect("fresh capital admission state and capacity should admit")
        .commit_submitted();
    (admission, feed)
}

fn capital_admission_submit_request(client_order_id: &str) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        execution_client_id: "execution-client-a".to_string(),
        client_order_id: client_order_id.to_string(),
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        notional: Decimal::new(4, 0),
        order_side: OrderSide::Buy,
        order_quantity: Decimal::new(10, 0),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        risk_reducing_exit_proof: None,
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
        provider_collateral_allowance: provider_collateral_allowance_snapshot(observed_at_ns, 100),
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
        provider_collateral_allowance: state.provider_collateral_allowance,
        order_lifecycle: state.order_lifecycle,
        product_state: state.product_state,
        loss_snapshot: state.loss_snapshot,
    }
}

fn seed_provider_collateral_allowance(feed: &mut CapitalAdmissionRuntimeFeed, observed_at_ns: u64) {
    let _ = feed.on_provider_collateral_allowance_snapshot(provider_collateral_allowance_snapshot(
        observed_at_ns,
        100,
    ));
}

fn provider_collateral_allowance_snapshot(
    observed_at_ns: u64,
    collateral_allowance: i64,
) -> ProviderCollateralAllowanceSnapshot {
    ProviderCollateralAllowanceSnapshot {
        source: "operator-venue-allowance".to_string(),
        observed_at_ns,
        venue_id: "VENUE-A".to_string(),
        account_id: "ACCOUNT-001".to_string(),
        collateral_currency: "USD".to_string(),
        collateral_allowance: Decimal::new(collateral_allowance, 0),
    }
}

fn polymarket_provider_collateral_allowance_snapshot(
    captured_at: u64,
    balance: Decimal,
    allowance: Decimal,
) -> ProviderCollateralAllowanceSnapshot {
    build_polymarket_provider_collateral_allowance_snapshot(
        PolymarketProviderCollateralAllowanceInput {
            captured_at: UnixNanos::from(captured_at),
            account_id: AccountId::from("ACCOUNT-001"),
            collateral_currency: Currency::from("USD"),
            collateral: BalanceAllowance {
                balance,
                allowance: Some(allowance),
            },
        },
    )
    .expect("test provider collateral allowance snapshot should be valid")
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
