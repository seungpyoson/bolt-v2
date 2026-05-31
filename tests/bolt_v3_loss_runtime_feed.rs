mod support;

use bolt_v2::bolt_v3_loss_governor::LossGovernorPolicy;
use bolt_v2::bolt_v3_loss_runtime_feed::{
    LossGovernorRuntimeFeed, LossGovernorRuntimeFeedConfig, subscribe_loss_governor_runtime_feed,
};
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
    BoltV3SubmitLifecyclePolicy,
};
use nautilus_common::msgbus::{publish_portfolio_snapshot, publish_position_event};
use nautilus_core::UUID4;
use nautilus_model::enums::{AccountType, PositionAdjustmentType};
use nautilus_model::events::{PortfolioSnapshot, PositionAdjusted, PositionEvent};
use nautilus_model::identifiers::{AccountId, InstrumentId, PositionId, StrategyId, TraderId};
use nautilus_model::types::{Currency, Money};
use rust_decimal::Decimal;
use std::sync::{Arc, Mutex};

#[test]
fn nt_runtime_feed_publishes_oldest_observed_loss_snapshot_to_submit_admission() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed_with_loss_governor(
        writer.clone(),
        loss_policy(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(5, 0),
        ))
        .expect("valid canary report should arm admission");

    let account_id = AccountId::from("SIM-LOSS-001");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
        },
        admission.clone(),
    );

    let portfolio_snapshot = portfolio_snapshot(account_id, 1_000, -4.0, -3.0, 1_000.0);
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot).is_none(),
        "portfolio facts alone must not publish a complete loss snapshot"
    );

    let position_event = adjusted_position_event(account_id, 900, -2.0);
    let snapshot = feed
        .on_position_event(&position_event)
        .expect("position facts should complete the NT-derived loss snapshot");

    assert_eq!(snapshot.observed_at_ns, 900);
    assert_eq!(snapshot.per_trade_pnl, Some(Decimal::new(-2, 0)));
    assert_eq!(snapshot.daily_pnl, Some(Decimal::new(-7, 0)));
    assert_eq!(snapshot.rolling_pnl, Some(Decimal::new(-2, 0)));
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
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed_with_loss_governor(
        writer,
        loss_policy(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(5, 0),
        ))
        .expect("valid canary report should arm admission");

    let account_id = AccountId::from("SIM-LOSS-002");
    let feed = Arc::new(Mutex::new(LossGovernorRuntimeFeed::new(
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
    publish_position_event(
        "events.position.STRATEGY-LOSS-001".into(),
        &adjusted_position_event(account_id, 1_900, -2.0),
    );
    subscription.unsubscribe_all();

    let snapshot = feed
        .lock()
        .expect("feed mutex should not be poisoned")
        .latest_snapshot()
        .cloned()
        .expect("subscribed NT events should publish a loss snapshot");
    assert_eq!(snapshot.observed_at_ns, 1_900);
    admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 2_100)
        .expect("subscribed NT-derived snapshot should admit entry submit");
}

#[test]
fn rolling_window_and_drawdown_facts_use_oldest_contributing_nt_timestamp() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed_with_loss_governor(
        writer,
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-003");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
        },
        admission,
    );

    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_000, -1.0, 0.0, 1_000.0))
            .is_none(),
        "portfolio facts alone must not publish a complete loss snapshot"
    );

    let first = feed
        .on_position_event(&adjusted_position_event(account_id, 900, -2.0))
        .expect("first rolling delta should complete the snapshot");
    assert_eq!(first.observed_at_ns, 900);
    assert_eq!(first.rolling_pnl, Some(Decimal::new(-2, 0)));

    let second = feed
        .on_position_event(&adjusted_position_event(account_id, 1_300, -3.0))
        .expect("later rolling delta should retain complete snapshot");
    assert_eq!(second.observed_at_ns, 1_000);
    assert_eq!(second.per_trade_pnl, Some(Decimal::new(-3, 0)));
    assert_eq!(second.rolling_pnl, Some(Decimal::new(-3, 0)));
    assert_eq!(second.current_equity, Some(Decimal::new(1_000, 0)));
    assert_eq!(second.peak_equity, Some(Decimal::new(1_000, 0)));

    let drawdown = feed
        .on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_400, -4.0, -3.0, 960.0))
        .expect("fresher portfolio facts should retain the older peak-equity timestamp");
    assert_eq!(drawdown.observed_at_ns, 1_000);
    assert_eq!(drawdown.daily_pnl, Some(Decimal::new(-7, 0)));
    assert_eq!(drawdown.rolling_pnl, Some(Decimal::new(-3, 0)));
    assert_eq!(drawdown.current_equity, Some(Decimal::new(960, 0)));
    assert_eq!(drawdown.peak_equity, Some(Decimal::new(1_000, 0)));
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
        client_order_id: "client-order-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        notional,
        intent_kind: BoltV3SubmitIntentKind::Entry,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
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
