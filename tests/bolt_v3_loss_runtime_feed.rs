mod support;

use bolt_v2::bolt_v3_loss_governor::{LossGovernorPolicy, LossSnapshot};
use bolt_v2::bolt_v3_loss_halt_actions::LossGovernorHaltActionHandler;
use bolt_v2::bolt_v3_loss_runtime_feed::{
    LossGovernorRuntimeFeed, LossGovernorRuntimeFeedConfig, subscribe_loss_governor_runtime_feed,
};
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
    BoltV3SubmitLifecyclePolicy,
};
use nautilus_common::msgbus::publish_portfolio_snapshot;
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::enums::{AccountType, OrderSide, PositionAdjustmentType, PositionSide};
use nautilus_model::events::{PortfolioSnapshot, PositionAdjusted, PositionChanged, PositionEvent};
use nautilus_model::identifiers::{
    AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId, TraderId,
};
use nautilus_model::types::{Currency, Money, Price, Quantity};
use rust_decimal::Decimal;
use std::{cell::RefCell, rc::Rc, sync::Arc};

#[test]
fn nt_runtime_feed_publishes_fresh_portfolio_loss_snapshot_to_submit_admission() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.clone(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-001");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
        },
        admission.clone(),
    );

    let portfolio_snapshot = portfolio_snapshot(account_id, 1_000, -4.0, -3.0, 1_000.0);
    let snapshot = feed
        .on_portfolio_snapshot(&portfolio_snapshot)
        .expect("portfolio facts should publish an NT-derived baseline loss snapshot");

    assert_eq!(snapshot.observed_at_ns, 1_000);
    assert_eq!(snapshot.per_trade_pnl, Some(Decimal::ZERO));
    assert_eq!(snapshot.daily_pnl, Some(Decimal::new(-7, 0)));
    assert_eq!(snapshot.rolling_pnl, Some(Decimal::ZERO));
    assert_eq!(snapshot.current_equity, Some(Decimal::new(1_000, 0)));
    assert_eq!(snapshot.peak_equity, Some(Decimal::new(1_000, 0)));

    admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 1_100)
        .expect("fresh below-limit NT-derived snapshot should admit entry submit");
    assert_eq!(admission.admitted_order_count(), 1);
    assert_eq!(writer.admission_decisions().len(), 1);
}

#[test]
fn subscribed_nt_events_update_submit_admission_loss_snapshot() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-002");
    let feed = Rc::new(RefCell::new(LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
        },
        admission.clone(),
    )));
    let mut subscription = subscribe_loss_governor_runtime_feed(feed.clone());

    publish_portfolio_snapshot(
        "events.portfolio.SIM-LOSS-002".into(),
        &portfolio_snapshot(account_id, 2_000, -4.0, -3.0, 1_000.0),
    );
    subscription.unsubscribe_all();

    let snapshot = feed
        .borrow()
        .latest_snapshot()
        .cloned()
        .expect("subscribed NT events should publish a loss snapshot");
    assert_eq!(snapshot.observed_at_ns, 2_000);
    admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 2_100)
        .expect("subscribed NT-derived snapshot should admit entry submit");
}

#[test]
fn rolling_window_advances_from_portfolio_pnl_deltas_and_evicts_on_heartbeat() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        LossGovernorPolicy {
            max_snapshot_age_ns: 1_000,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_session_loss: Some(Decimal::new(100, 0)),
            max_rolling_loss: Some(Decimal::new(30, 0)),
            max_drawdown: Some(Decimal::new(100, 0)),
        },
    ));
    let account_id = AccountId::from("SIM-LOSS-003");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
        },
        admission,
    );

    let baseline = feed
        .on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_000, 0.0, 0.0, 1_000.0))
        .expect("portfolio baseline should publish");
    assert_eq!(baseline.observed_at_ns, 1_000);
    assert_eq!(baseline.rolling_pnl, Some(Decimal::ZERO));

    let breached = feed
        .on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_100, -35.0, 0.0, 965.0))
        .expect("portfolio pnl delta should publish rolling loss");
    assert_eq!(breached.observed_at_ns, 1_100);
    assert_eq!(breached.rolling_pnl, Some(Decimal::new(-35, 0)));

    let recovered = feed
        .on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_401, -35.0, 0.0, 965.0))
        .expect("portfolio heartbeat should evict expired rolling loss");
    assert_eq!(recovered.observed_at_ns, 1_401);
    assert_eq!(recovered.rolling_pnl, Some(Decimal::ZERO));
}

#[test]
fn position_adjustment_does_not_mask_larger_per_trade_loss() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-004");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
        },
        admission,
    );

    feed.on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_000, 0.0, 0.0, 1_000.0))
        .expect("portfolio baseline should publish");
    let position_loss = feed
        .on_position_event(&changed_position_event(account_id, 1_100, -8.0))
        .expect("position changed should publish per-trade pnl");
    assert_eq!(position_loss.per_trade_pnl, Some(Decimal::new(-8, 0)));

    let commission_adjustment = feed
        .on_position_event(&adjusted_position_event(account_id, 1_200, -1.0))
        .expect("position adjustment should retain the last trade-level pnl");
    assert_eq!(
        commission_adjustment.per_trade_pnl,
        Some(Decimal::new(-8, 0))
    );
}

#[test]
fn stale_peak_timestamp_does_not_make_fresh_portfolio_snapshot_stale() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        LossGovernorPolicy {
            max_snapshot_age_ns: 100,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_session_loss: Some(Decimal::new(25, 0)),
            max_rolling_loss: Some(Decimal::new(30, 0)),
            max_drawdown: Some(Decimal::new(100, 0)),
        },
    ));
    let account_id = AccountId::from("SIM-LOSS-005");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
        },
        admission.clone(),
    );

    feed.on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_000, 0.0, 0.0, 1_000.0))
        .expect("initial peak should publish");
    let fresh_drawdown = feed
        .on_portfolio_snapshot(&portfolio_snapshot(account_id, 3_000, -4.0, -3.0, 960.0))
        .expect("fresh portfolio heartbeat should publish despite older peak value");
    assert_eq!(fresh_drawdown.observed_at_ns, 3_000);
    assert_eq!(fresh_drawdown.peak_equity, Some(Decimal::new(1_000, 0)));

    admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 3_050)
        .expect("fresh below-limit drawdown snapshot should admit entry submit");
}

#[test]
fn feed_fails_closed_on_mixed_currency_portfolio_without_base_currency() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-006");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
        },
        admission,
    );

    // No base currency, with money facts spanning two currencies: the feed cannot
    // resolve a single account currency and must publish nothing.
    assert!(
        feed.on_portfolio_snapshot(&mixed_currency_portfolio_snapshot(account_id, 1_000))
            .is_none()
    );
    assert!(feed.latest_snapshot().is_none());
}

#[test]
fn feed_fails_closed_on_empty_portfolio_money_facts() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-007");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
        },
        admission,
    );

    // A snapshot with no money facts for the account currency must fail closed
    // rather than treat the missing values as zero.
    assert!(
        feed.on_portfolio_snapshot(&empty_money_portfolio_snapshot(account_id, 1_000))
            .is_none()
    );
    assert!(feed.latest_snapshot().is_none());
}

#[test]
fn feed_fails_closed_on_mixed_currency_position_pnl() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-008");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
        },
        admission,
    );

    // realized PnL in a different currency than unrealized PnL cannot be combined
    // into a single trade-level figure, so the event must be dropped fail-closed.
    assert!(
        feed.on_position_event(&mixed_currency_changed_position_event(account_id, 1_000))
            .is_none()
    );
    assert!(feed.latest_snapshot().is_none());
}

#[test]
fn published_snapshot_invokes_configured_halt_action_handler() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-009");

    let invocations: Rc<RefCell<Vec<(u64, Option<Decimal>)>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = invocations.clone();
    let handler: LossGovernorHaltActionHandler = Rc::new(
        move |snapshot: Option<&LossSnapshot>, observed_at_ns: u64| {
            recorder.borrow_mut().push((
                observed_at_ns,
                snapshot.and_then(|snapshot| snapshot.daily_pnl),
            ));
        },
    );
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
        },
        admission,
    )
    .with_halt_action_handler(handler);

    // A complete portfolio snapshot is published; the configured halt-action
    // handler must fire with that snapshot so the loss-governor trigger path
    // (feed -> handler) is exercised end to end.
    let published = feed
        .on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_000, -50.0, 0.0, 950.0))
        .expect("complete portfolio facts should publish a loss snapshot");

    // Pin the concrete propagated facts independently so the wiring assertion
    // below is not self-referential: daily PnL is realized(-50) + unrealized(0)
    // and observed_at_ns is the min over all four facts, which share ts_event.
    assert_eq!(published.observed_at_ns, 1_000);
    assert_eq!(published.daily_pnl, Some(Decimal::new(-50, 0)));

    let recorded = invocations.borrow();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0], (1_000, Some(Decimal::new(-50, 0))));
}

fn loss_policy() -> LossGovernorPolicy {
    LossGovernorPolicy {
        max_snapshot_age_ns: 1_000,
        max_per_trade_loss: Some(Decimal::new(10, 0)),
        max_session_loss: Some(Decimal::new(25, 0)),
        max_rolling_loss: Some(Decimal::new(30, 0)),
        max_drawdown: Some(Decimal::new(40, 0)),
    }
}

fn submit_request(notional: Decimal) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        execution_client_id: "execution-client-a".to_string(),
        client_order_id: "client-order-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        notional,
        order_side: OrderSide::Buy,
        order_quantity: Decimal::new(1, 0),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_proof: None,
        kill_switch_forced_reduction: None,
        position_sizing: None,
    }
}

fn portfolio_snapshot(
    account_id: AccountId,
    ts_event: u64,
    realized_pnl: f64,
    unrealized_pnl: f64,
    total_equity: f64,
) -> PortfolioSnapshot {
    PortfolioSnapshot::new(
        account_id,
        AccountType::Cash,
        Some(Currency::USD()),
        vec![],
        vec![],
        vec![Money::new(unrealized_pnl, Currency::USD())],
        vec![Money::new(realized_pnl, Currency::USD())],
        vec![Money::new(total_equity, Currency::USD())],
        UUID4::default(),
        ts_event.into(),
        ts_event.into(),
    )
}

fn adjusted_position_event(account_id: AccountId, ts_event: u64, pnl_change: f64) -> PositionEvent {
    PositionEvent::PositionAdjusted(PositionAdjusted::new(
        TraderId::from("TRADER-LOSS-001"),
        StrategyId::from("STRATEGY-LOSS-001"),
        InstrumentId::from("INSTRUMENT-LOSS-001.SIM"),
        PositionId::from("POSITION-LOSS-001"),
        account_id,
        PositionAdjustmentType::Commission,
        None,
        Some(Money::new(pnl_change, Currency::USD())),
        None,
        UUID4::default(),
        ts_event.into(),
        ts_event.into(),
    ))
}

fn changed_position_event(
    account_id: AccountId,
    ts_event: u64,
    unrealized_pnl: f64,
) -> PositionEvent {
    PositionEvent::PositionChanged(PositionChanged {
        trader_id: TraderId::from("TRADER-LOSS-001"),
        strategy_id: StrategyId::from("STRATEGY-LOSS-001"),
        instrument_id: InstrumentId::from("INSTRUMENT-LOSS-001.SIM"),
        position_id: PositionId::from("POSITION-LOSS-001"),
        account_id,
        opening_order_id: ClientOrderId::from("ORDER-LOSS-001"),
        entry: OrderSide::Buy,
        side: PositionSide::Long,
        signed_qty: 1.0,
        quantity: Quantity::from("1"),
        peak_quantity: Quantity::from("1"),
        last_qty: Quantity::from("1"),
        last_px: Price::from("1.00"),
        currency: Currency::USD(),
        avg_px_open: 1.0,
        avg_px_close: None,
        realized_return: 0.0,
        realized_pnl: None,
        unrealized_pnl: Money::new(unrealized_pnl, Currency::USD()),
        event_id: UUID4::default(),
        ts_opened: UnixNanos::from(1),
        ts_event: UnixNanos::from(ts_event),
        ts_init: UnixNanos::from(ts_event),
    })
}

fn mixed_currency_portfolio_snapshot(account_id: AccountId, ts_event: u64) -> PortfolioSnapshot {
    PortfolioSnapshot::new(
        account_id,
        AccountType::Cash,
        None,
        vec![],
        vec![],
        vec![Money::new(-3.0, Currency::USD())],
        vec![Money::new(-4.0, Currency::USD())],
        vec![Money::new(1_000.0, Currency::EUR())],
        UUID4::default(),
        ts_event.into(),
        ts_event.into(),
    )
}

fn empty_money_portfolio_snapshot(account_id: AccountId, ts_event: u64) -> PortfolioSnapshot {
    PortfolioSnapshot::new(
        account_id,
        AccountType::Cash,
        Some(Currency::USD()),
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        UUID4::default(),
        ts_event.into(),
        ts_event.into(),
    )
}

fn mixed_currency_changed_position_event(account_id: AccountId, ts_event: u64) -> PositionEvent {
    PositionEvent::PositionChanged(PositionChanged {
        trader_id: TraderId::from("TRADER-LOSS-001"),
        strategy_id: StrategyId::from("STRATEGY-LOSS-001"),
        instrument_id: InstrumentId::from("INSTRUMENT-LOSS-001.SIM"),
        position_id: PositionId::from("POSITION-LOSS-001"),
        account_id,
        opening_order_id: ClientOrderId::from("ORDER-LOSS-001"),
        entry: OrderSide::Buy,
        side: PositionSide::Long,
        signed_qty: 1.0,
        quantity: Quantity::from("1"),
        peak_quantity: Quantity::from("1"),
        last_qty: Quantity::from("1"),
        last_px: Price::from("1.00"),
        currency: Currency::USD(),
        avg_px_open: 1.0,
        avg_px_close: None,
        realized_return: 0.0,
        realized_pnl: Some(Money::new(-4.0, Currency::EUR())),
        unrealized_pnl: Money::new(-3.0, Currency::USD()),
        event_id: UUID4::default(),
        ts_opened: UnixNanos::from(1),
        ts_event: UnixNanos::from(ts_event),
        ts_init: UnixNanos::from(ts_event),
    })
}
