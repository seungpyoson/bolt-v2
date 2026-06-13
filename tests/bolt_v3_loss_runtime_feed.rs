mod support;

use bolt_v2::bolt_v3_loss_governor::{LossGovernorPolicy, LossHaltReason};
use bolt_v2::bolt_v3_loss_halt_actions::LossGovernorHaltActionHandler;
use bolt_v2::bolt_v3_loss_runtime_feed::{
    LossGovernorRuntimeFeed, LossGovernorRuntimeFeedConfig, subscribe_loss_governor_runtime_feed,
};
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
    BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy,
};
use nautilus_common::msgbus::{publish_account_state, publish_portfolio_snapshot};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::enums::{AccountType, OrderSide, PositionAdjustmentType, PositionSide};
use nautilus_model::events::{
    AccountState, PortfolioSnapshot, PositionAdjusted, PositionChanged, PositionEvent,
};
use nautilus_model::identifiers::{
    AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId, TraderId,
};
use nautilus_model::types::{AccountBalance, Currency, Money, Price, Quantity};
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
fn subscribed_account_state_without_portfolio_snapshot_updates_loss_snapshot() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-ACCOUNT");
    let feed = Rc::new(RefCell::new(LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
        },
        admission.clone(),
    )));
    let mut subscription = subscribe_loss_governor_runtime_feed(feed.clone());

    publish_account_state(
        "events.account.SIM-LOSS-ACCOUNT".into(),
        &account_state(account_id, 2_000, 1_000.0),
    );
    subscription.unsubscribe_all();

    let snapshot =
        feed.borrow().latest_snapshot().cloned().expect(
            "subscribed account state should publish a loss snapshot without portfolio timer",
        );
    assert_eq!(snapshot.observed_at_ns, 2_000);
    assert_eq!(snapshot.per_trade_pnl, Some(Decimal::ZERO));
    assert_eq!(snapshot.daily_pnl, Some(Decimal::ZERO));
    assert_eq!(snapshot.rolling_pnl, Some(Decimal::ZERO));
    assert_eq!(snapshot.current_equity, Some(Decimal::new(1_000, 0)));
    assert_eq!(snapshot.peak_equity, Some(Decimal::new(1_000, 0)));
    admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 2_100)
        .expect("account-state-derived loss snapshot should admit entry submit");
}

#[test]
fn account_state_equity_drop_updates_daily_and_rolling_loss() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-ACCOUNT-DROP");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
        },
        admission.clone(),
    );

    feed.on_account_state(&account_state(account_id, 1_000, 1_000.0))
        .expect("account state baseline should publish");
    let snapshot = feed
        .on_account_state(&account_state(account_id, 1_100, 960.0))
        .expect("lower account equity should publish account-state loss snapshot");

    assert_eq!(snapshot.observed_at_ns, 1_100);
    assert_eq!(snapshot.per_trade_pnl, Some(Decimal::ZERO));
    assert_eq!(snapshot.daily_pnl, Some(Decimal::new(-40, 0)));
    assert_eq!(snapshot.rolling_pnl, Some(Decimal::new(-40, 0)));
    assert_eq!(snapshot.current_equity, Some(Decimal::new(960, 0)));
    assert_eq!(snapshot.peak_equity, Some(Decimal::new(1_000, 0)));

    let error = admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 1_150)
        .expect_err("account-state loss beyond policy should halt entry submit");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons == vec![
                LossHaltReason::DailyLossLimit,
                LossHaltReason::RollingLossLimit,
                LossHaltReason::MaxDrawdownLimit,
            ]
    ));
}

#[test]
fn account_state_heartbeat_preserves_portfolio_loss_components() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-ACCOUNT-PRESERVE");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
        },
        admission.clone(),
    );

    feed.on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_000, 0.0, 0.0, 1_000.0))
        .expect("portfolio baseline should publish");
    feed.on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_100, -20.0, -20.0, 960.0))
        .expect("portfolio loss should publish");
    let snapshot = feed
        .on_account_state(&account_state(account_id, 1_200, 959.0))
        .expect("account heartbeat should refresh without erasing portfolio loss evidence");

    assert_eq!(snapshot.observed_at_ns, 1_200);
    assert_eq!(snapshot.per_trade_pnl, Some(Decimal::ZERO));
    assert_eq!(snapshot.daily_pnl, Some(Decimal::new(-40, 0)));
    assert_eq!(snapshot.rolling_pnl, Some(Decimal::new(-40, 0)));
    assert_eq!(snapshot.current_equity, Some(Decimal::new(959, 0)));
    assert_eq!(snapshot.peak_equity, Some(Decimal::new(1_000, 0)));

    let error = admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 1_250)
        .expect_err("account heartbeat must not clear portfolio-proven loss halt");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons == vec![
                LossHaltReason::DailyLossLimit,
                LossHaltReason::RollingLossLimit,
                LossHaltReason::MaxDrawdownLimit,
            ]
    ));
}

#[test]
fn halt_action_handler_receives_snapshot_init_time_as_now() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-HALT-INIT");
    let calls = Rc::new(RefCell::new(Vec::new()));
    let handler_calls = Rc::clone(&calls);
    let handler: LossGovernorHaltActionHandler = Rc::new(move |snapshot, now_ns| {
        handler_calls
            .borrow_mut()
            .push((snapshot.map(|snapshot| snapshot.observed_at_ns), now_ns));
    });
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
        },
        admission,
    )
    .with_halt_action_handler(handler);

    feed.on_portfolio_snapshot(&portfolio_snapshot_with_init(
        account_id, 1_000, 2_000, -4.0, -3.0, 1_000.0,
    ))
    .expect("valid portfolio facts should publish a loss snapshot");

    assert_eq!(
        calls.borrow().as_slice(),
        &[(Some(1_000), 2_000)],
        "halt actions must evaluate freshness at NT init time, not at the snapshot observation time"
    );
}

#[test]
fn subscribed_untrusted_portfolio_snapshot_invokes_halt_action_with_none() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-UNTRUSTED");
    let calls = Rc::new(RefCell::new(Vec::new()));
    let handler_calls = Rc::clone(&calls);
    let handler: LossGovernorHaltActionHandler = Rc::new(move |snapshot, now_ns| {
        handler_calls
            .borrow_mut()
            .push((snapshot.is_none(), now_ns));
    });
    let feed = Rc::new(RefCell::new(
        LossGovernorRuntimeFeed::new(
            LossGovernorRuntimeFeedConfig {
                account_id,
                rolling_window_ns: 500,
            },
            admission,
        )
        .with_halt_action_handler(handler),
    ));
    let mut subscription = subscribe_loss_governor_runtime_feed(feed);

    publish_portfolio_snapshot(
        "events.portfolio.SIM-LOSS-UNTRUSTED".into(),
        &portfolio_snapshot_without_pnl(account_id, 1_000, 2_000, 1_000.0),
    );
    subscription.unsubscribe_all();

    assert_eq!(
        calls.borrow().as_slice(),
        &[(true, 2_000)],
        "same-account malformed NT loss evidence must trigger the untrusted-snapshot action path"
    );
}

#[test]
fn rolling_window_advances_from_portfolio_pnl_deltas_and_evicts_on_heartbeat() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        LossGovernorPolicy {
            max_snapshot_age_ns: 1_000,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_daily_loss: Some(Decimal::new(100, 0)),
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
fn account_state_heartbeat_refreshes_position_event_per_trade_timestamp() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-PER-TRADE-FRESH");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
        },
        admission.clone(),
    );

    feed.on_account_state(&account_state(account_id, 1_000, 1_000.0))
        .expect("account baseline should publish");
    let position_loss = feed
        .on_position_event(&changed_position_event(account_id, 1_100, -8.0))
        .expect("position changed should publish per-trade pnl");
    assert_eq!(position_loss.per_trade_pnl, Some(Decimal::new(-8, 0)));

    let heartbeat = feed
        .on_account_state(&account_state(account_id, 2_000, 1_000.0))
        .expect("account heartbeat should keep position-event pnl fresh");
    assert_eq!(heartbeat.observed_at_ns, 2_000);
    assert_eq!(heartbeat.per_trade_pnl, Some(Decimal::new(-8, 0)));

    admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 2_200)
        .expect("fresh below-limit position-event pnl should admit entry submit");
}

#[test]
fn stale_peak_timestamp_does_not_make_fresh_portfolio_snapshot_stale() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer,
        LossGovernorPolicy {
            max_snapshot_age_ns: 100,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_daily_loss: Some(Decimal::new(25, 0)),
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

fn loss_policy() -> LossGovernorPolicy {
    LossGovernorPolicy {
        max_snapshot_age_ns: 1_000,
        max_per_trade_loss: Some(Decimal::new(10, 0)),
        max_daily_loss: Some(Decimal::new(25, 0)),
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

fn account_state(account_id: AccountId, ts_event: u64, total_equity: f64) -> AccountState {
    AccountState::new(
        account_id,
        AccountType::Cash,
        vec![AccountBalance::new(
            Money::new(total_equity, Currency::USD()),
            Money::new(0.0, Currency::USD()),
            Money::new(total_equity, Currency::USD()),
        )],
        vec![],
        true,
        UUID4::default(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        Some(Currency::USD()),
    )
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

fn portfolio_snapshot_with_init(
    account_id: AccountId,
    ts_event: u64,
    ts_init: u64,
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
        ts_init.into(),
    )
}

fn portfolio_snapshot_without_pnl(
    account_id: AccountId,
    ts_event: u64,
    ts_init: u64,
    total_equity: f64,
) -> PortfolioSnapshot {
    PortfolioSnapshot::new(
        account_id,
        AccountType::Cash,
        Some(Currency::USD()),
        vec![],
        vec![],
        vec![],
        vec![],
        vec![Money::new(total_equity, Currency::USD())],
        UUID4::default(),
        ts_event.into(),
        ts_init.into(),
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
