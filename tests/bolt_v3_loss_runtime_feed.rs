use crate::support;

use bolt_v2::bolt_v3_current_evidence::{
    AdmissionDecisionOutcome, AdmissionRejectionReason, CurrentFact, LossSnapshotSource,
    LossSnapshotStaleReason, StaleLossReason,
};
use bolt_v2::bolt_v3_loss_governor::{
    LossGovernorPolicy, LossHaltReason, LossSnapshot, LossSourceObservationTimestamps,
};
use bolt_v2::bolt_v3_loss_halt_actions::LossGovernorHaltActionHandler;
use bolt_v2::bolt_v3_loss_runtime_feed::{
    LossGovernorRuntimeFeed, LossGovernorRuntimeFeedConfig, subscribe_loss_governor_runtime_feed,
};
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
    BoltV3SubmitIntentKind,
};
use nautilus_common::msgbus::{publish_account_state, publish_portfolio_snapshot};
use nautilus_core::{UUID4, UnixNanos, nanos::DurationNanos};
use nautilus_model::enums::{AccountType, OrderSide, PositionAdjustmentType, PositionSide};
use nautilus_model::events::{
    AccountState, PortfolioSnapshot, PositionAdjusted, PositionChanged, PositionClosed,
    PositionEvent, PositionOpened,
};
use nautilus_model::identifiers::{
    AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId, TraderId,
};
use nautilus_model::types::{AccountBalance, Currency, Money, Price, Quantity};
use rust_decimal::Decimal;
use std::{cell::RefCell, rc::Rc, sync::Arc};

#[test]
fn nt_runtime_feed_publishes_fresh_portfolio_loss_snapshot_to_submit_admission() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-001");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
            active_position_pnl_max_entries: 64,
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
        .expect("fresh below-limit NT-derived snapshot should admit entry submit")
        .commit_submitted();
    assert_eq!(admission.admitted_order_count(), 1);
    assert_eq!(writer.admission_decisions().len(), 1);
}

#[test]
fn subscribed_nt_events_update_submit_admission_loss_snapshot() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-002");
    let feed = Rc::new(RefCell::new(LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
            active_position_pnl_max_entries: 64,
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
        .expect("subscribed NT-derived snapshot should admit entry submit")
        .commit_submitted();
}

#[test]
fn subscribed_account_state_without_portfolio_snapshot_updates_loss_snapshot() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-ACCOUNT");
    let feed = Rc::new(RefCell::new(LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
            active_position_pnl_max_entries: 64,
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
        .expect("account-state-derived loss snapshot should admit entry submit")
        .commit_submitted();
}

#[test]
fn account_state_equity_drop_updates_daily_and_rolling_loss() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-ACCOUNT-DROP");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
            active_position_pnl_max_entries: 64,
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-ACCOUNT-PRESERVE");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
            active_position_pnl_max_entries: 64,
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-HALT-INIT");
    let calls = Rc::new(RefCell::new(Vec::new()));
    let handler_calls = Rc::clone(&calls);
    let handler: LossGovernorHaltActionHandler = Rc::new(move |snapshot, now_ns, _| {
        handler_calls
            .borrow_mut()
            .push((snapshot.map(|snapshot| snapshot.observed_at_ns), now_ns));
    });
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
            active_position_pnl_max_entries: 64,
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-UNTRUSTED");
    let calls = Rc::new(RefCell::new(Vec::new()));
    let handler_calls = Rc::clone(&calls);
    let handler: LossGovernorHaltActionHandler = Rc::new(move |snapshot, now_ns, observations| {
        handler_calls.borrow_mut().push((
            snapshot.is_none(),
            now_ns,
            observations.last_portfolio_snapshot_observed_at_ns,
        ));
    });
    let feed = Rc::new(RefCell::new(
        LossGovernorRuntimeFeed::new(
            LossGovernorRuntimeFeedConfig {
                account_id,
                rolling_window_ns: 500,
                active_position_pnl_max_entries: 64,
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
        &[(true, 2_000, Some(1_000))],
        "same-account malformed NT loss evidence must trigger the untrusted-snapshot action path"
    );
}

#[test]
fn stale_snapshot_admission_records_real_source_observation_diagnostics() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-STALE-DIAG");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
            active_position_pnl_max_entries: 64,
        },
        admission.clone(),
    );

    feed.on_account_state(&account_state(account_id, 1_000, 1_000.0))
        .expect("account state should publish a baseline loss snapshot");
    feed.on_portfolio_snapshot(&portfolio_snapshot(account_id, 2_000, 0.0, 0.0, 1_000.0))
        .expect("portfolio snapshot should publish loss snapshot facts");
    feed.on_position_event(&changed_position_event(account_id, 2_500, -1.0))
        .expect("position event should publish per-trade loss fact");

    let stale_error = admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 3_501)
        .expect_err("stale loss snapshot should reject entry admission");
    assert!(matches!(
        stale_error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons == vec![LossHaltReason::StaleLossSnapshot]
    ));

    let decisions = writer.admission_decisions();
    assert_eq!(decisions.len(), 1);
    let decision = &decisions[0];
    assert_eq!(
        decision.outcome,
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::LossGovernorHalted)
    );
    assert_eq!(
        decision.stale_reason,
        Some(LossSnapshotStaleReason::AgeExceeded)
    );
    assert_eq!(decision.snapshot_age_ns, Some(1_501));
    assert_eq!(decision.max_snapshot_age_ns, Some(1_000));
    assert_eq!(
        decision.snapshot_source,
        Some(LossSnapshotSource::NtLossRuntimeFeed)
    );
    assert!(decision.per_trade_pnl_present);
    assert!(decision.daily_pnl_present);
    assert!(decision.rolling_pnl_present);
    assert!(decision.current_equity_present);
    assert!(decision.peak_equity_present);
    assert_eq!(decision.last_account_state_observed_at_ns, Some(1_000));
    assert_eq!(decision.last_portfolio_snapshot_observed_at_ns, Some(2_000));
    assert_eq!(decision.last_position_event_observed_at_ns, Some(2_500));
}

#[test]
fn stale_loss_halt_emits_populated_loss_governor_halt_evidence() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::new();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-RCA-EVIDENCE");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
            active_position_pnl_max_entries: 64,
        },
        admission.clone(),
    );

    assert!(
        feed.on_position_event(&changed_position_event(account_id, 1_600, -1.0))
            .is_none(),
        "position-only raw facts should not publish a complete loss snapshot"
    );
    assert!(
        feed.on_portfolio_snapshot(&portfolio_snapshot_without_pnl(
            account_id, 1_500, 1_500, 1_000.0,
        ))
        .is_none(),
        "malformed portfolio raw facts should update freshness without publishing"
    );
    feed.on_account_state(&account_state(account_id, 1_700, 1_000.0))
        .expect(
            "account facts complete the feed state before the stale manual snapshot is installed",
        );
    admission.update_loss_snapshot(loss_snapshot_at(1_000));

    let error = admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 2_101)
        .expect_err("aged loss snapshot should halt entry submit");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons == vec![LossHaltReason::StaleLossSnapshot]
    ));

    let records = writer.loss_governor_halts();
    assert_eq!(records.len(), 1);
    let evidence = &records[0];
    assert_eq!(evidence.stale_reason, StaleLossReason::AgeExceeded);
    assert!(evidence.snapshot_present);
    assert_eq!(evidence.snapshot_observed_at_ns, Some(1_000));
    assert_eq!(evidence.admission_now_ns, 2_101);
    assert_eq!(evidence.snapshot_age_ns, Some(1_101));
    assert_eq!(evidence.max_snapshot_age_ns, 1_000);
    assert_eq!(
        evidence.snapshot_source.as_deref(),
        Some("nt_loss_runtime_feed")
    );
    assert!(evidence.has_per_trade_pnl);
    assert!(evidence.has_daily_pnl);
    assert!(evidence.has_rolling_pnl);
    assert!(evidence.has_current_equity);
    assert!(evidence.has_peak_equity);
    assert_eq!(evidence.account_state_count, 1);
    assert_eq!(evidence.portfolio_snapshot_count, 1);
    assert_eq!(evidence.position_event_count, 1);
    assert_eq!(evidence.last_account_state_ts_ns, Some(1_700));
    assert_eq!(evidence.last_portfolio_snapshot_ts_ns, Some(1_500));
    assert_eq!(evidence.last_position_event_ts_ns, Some(1_600));
    assert_eq!(
        evidence.stable_halt_key,
        "age_exceeded:nt_loss_runtime_feed"
    );
    assert_eq!(evidence.retry_count, 1);
    assert_eq!(evidence.elapsed_since_first_halt_ns, 0);
    assert!(writer.facts().into_iter().all(|fact| matches!(
        fact,
        CurrentFact::LossGovernorHalt(_)
            | CurrentFact::RejectedEntryAdmission(_)
            | CurrentFact::OrderReject(_)
    )));
}

#[test]
fn stale_loss_halt_evidence_exponentially_samples_and_resets_after_accept() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::new();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    admission.update_loss_snapshot(loss_snapshot_at(1_000));

    for attempt in 1..=20 {
        let now_ns = 2_100 + attempt;
        let error = admission
            .admit_at(&submit_request(Decimal::new(1, 0)), now_ns)
            .expect_err("identical stale loss snapshots should keep halting");
        assert!(matches!(
            error,
            BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
                if reasons == vec![LossHaltReason::StaleLossSnapshot]
        ));
    }

    let records = writer.loss_governor_halts();
    let retry_counts: Vec<u32> = records.iter().map(|record| record.retry_count).collect();
    assert_eq!(retry_counts, vec![1, 2, 4, 8, 16]);
    assert!(
        records
            .iter()
            .all(|record| record.stable_halt_key == "age_exceeded:nt_loss_runtime_feed")
    );

    admission.update_loss_snapshot(loss_snapshot_at(3_000));
    admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 3_100)
        .expect("fresh loss snapshot should reset stale-halt sampling")
        .commit_submitted();
    admission.update_loss_snapshot(loss_snapshot_at(3_000));
    admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 4_001)
        .expect_err("recurring stale loss snapshot should restart sampling at one");

    let reset_records = writer.loss_governor_halts();
    assert_eq!(reset_records.len(), 6);
    assert_eq!(reset_records[5].retry_count, 1);
    assert_eq!(reset_records[5].elapsed_since_first_halt_ns, 0);
}

#[test]
fn freshness_counts_advance_when_raw_events_do_not_publish_loss_snapshot() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::new();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let stale_snapshot = loss_snapshot_at(1_000);
    admission.update_loss_snapshot(stale_snapshot.clone());
    let account_id = AccountId::from("SIM-LOSS-FRESHNESS-DECOUPLED");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
            active_position_pnl_max_entries: 64,
        },
        admission.clone(),
    );

    assert!(
        feed.on_position_event(&changed_position_event(account_id, 1_500, -1.0))
            .is_none(),
        "a raw position event alone must not publish a complete loss snapshot"
    );
    assert!(
        feed.latest_snapshot().is_none(),
        "publish_if_complete should remain blocked without equity and rolling facts"
    );
    assert_eq!(
        admission.loss_snapshot(),
        Some(stale_snapshot),
        "raw freshness updates must not replace the last published loss snapshot"
    );

    admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 2_101)
        .expect_err("unchanged aged loss snapshot should still halt");

    let records = writer.loss_governor_halts();
    assert_eq!(records.len(), 1);
    let evidence = &records[0];
    assert_eq!(evidence.snapshot_observed_at_ns, Some(1_000));
    assert_eq!(evidence.position_event_count, 1);
    assert_eq!(evidence.last_position_event_ts_ns, Some(1_500));
    assert_eq!(evidence.account_state_count, 0);
    assert_eq!(evidence.last_account_state_ts_ns, None);
    assert_eq!(evidence.portfolio_snapshot_count, 0);
    assert_eq!(evidence.last_portfolio_snapshot_ts_ns, None);
}

#[test]
fn rolling_window_advances_from_portfolio_pnl_deltas_and_evicts_on_heartbeat() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
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
            active_position_pnl_max_entries: 64,
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-004");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
            active_position_pnl_max_entries: 64,
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
fn later_safe_position_event_does_not_mask_open_position_per_trade_loss() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-PER-TRADE-WORST");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
            active_position_pnl_max_entries: 64,
        },
        admission.clone(),
    );

    feed.on_account_state(&account_state(account_id, 1_000, 1_000.0))
        .expect("account baseline should publish");
    feed.on_position_event(&changed_position_event_for_position(
        account_id,
        "POSITION-LOSS-A",
        "INSTRUMENT-LOSS-A.SIM",
        1_100,
        -12.0,
    ))
    .expect("first open position should publish breaching per-trade pnl");
    let safe_second_position = feed
        .on_position_event(&changed_position_event_for_position(
            account_id,
            "POSITION-LOSS-B",
            "INSTRUMENT-LOSS-B.SIM",
            1_200,
            -1.0,
        ))
        .expect("later safe position should publish without masking the open breach");

    assert_eq!(
        safe_second_position.per_trade_pnl,
        Some(Decimal::new(-12, 0))
    );
    let error = admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 1_250)
        .expect_err("worst open position loss should halt entry submit");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons == vec![LossHaltReason::PerTradeLossLimit]
    ));
}

#[test]
fn active_position_pnl_cap_preserves_worst_overflow_loss() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-PER-TRADE-CAPPED");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
            active_position_pnl_max_entries: 1,
        },
        admission.clone(),
    );

    feed.on_account_state(&account_state(account_id, 1_000, 1_000.0))
        .expect("account baseline should publish");
    feed.on_position_event(&changed_position_event_for_position(
        account_id,
        "POSITION-LOSS-A",
        "INSTRUMENT-LOSS-A.SIM",
        1_100,
        -8.0,
    ))
    .expect("first open position should publish per-trade pnl");
    let safe_overflow_position = feed
        .on_position_event(&changed_position_event_for_position(
            account_id,
            "POSITION-LOSS-B",
            "INSTRUMENT-LOSS-B.SIM",
            1_200,
            -1.0,
        ))
        .expect("overflow position should not mask worse retained loss");
    assert_eq!(
        safe_overflow_position.per_trade_pnl,
        Some(Decimal::new(-8, 0))
    );

    let worse_overflow_position = feed
        .on_position_event(&changed_position_event_for_position(
            account_id,
            "POSITION-LOSS-C",
            "INSTRUMENT-LOSS-C.SIM",
            1_300,
            -12.0,
        ))
        .expect("worse overflow position should update conservative floor");
    assert_eq!(
        worse_overflow_position.per_trade_pnl,
        Some(Decimal::new(-12, 0))
    );
    let error = admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 1_350)
        .expect_err("worst overflow position loss should halt entry submit");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons == vec![LossHaltReason::PerTradeLossLimit]
    ));
}

#[test]
fn position_opened_resets_completed_position_per_trade_pnl() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-PER-TRADE-OPENED");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
            active_position_pnl_max_entries: 64,
        },
        admission.clone(),
    );

    feed.on_account_state(&account_state(account_id, 1_000, 1_000.0))
        .expect("account baseline should publish");
    feed.on_position_event(&changed_position_event_for_position(
        account_id,
        "POSITION-LOSS-CLOSED",
        "INSTRUMENT-LOSS-CLOSED.SIM",
        1_100,
        -8.0,
    ))
    .expect("changed position should publish per-trade pnl");
    feed.on_position_event(&closed_position_event(
        account_id,
        "POSITION-LOSS-CLOSED",
        "INSTRUMENT-LOSS-CLOSED.SIM",
        1_200,
        -8.0,
    ))
    .expect("closed position should publish final per-trade pnl");
    let opened = feed
        .on_position_event(&opened_position_event(
            account_id,
            "POSITION-LOSS-NEW",
            "INSTRUMENT-LOSS-NEW.SIM",
            1_300,
        ))
        .expect("opened position should reset completed per-trade pnl");

    assert_eq!(opened.per_trade_pnl, Some(Decimal::ZERO));
    admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 1_350)
        .expect("fresh new position baseline should admit entry submit")
        .commit_submitted();
}

#[test]
fn account_state_heartbeat_refreshes_position_event_per_trade_timestamp() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-PER-TRADE-FRESH");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
            active_position_pnl_max_entries: 64,
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
        .expect("fresh below-limit position-event pnl should admit entry submit")
        .commit_submitted();
}

#[test]
fn stale_peak_timestamp_does_not_make_fresh_portfolio_snapshot_stale() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
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
            active_position_pnl_max_entries: 64,
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
        .expect("fresh below-limit drawdown snapshot should admit entry submit")
        .commit_submitted();
}

#[test]
fn flat_daily_pnl_portfolio_snapshot_does_not_lower_peak_equity() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-DRAWDOWN-PRESERVE");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 500,
            active_position_pnl_max_entries: 64,
        },
        admission.clone(),
    );

    feed.on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_000, 0.0, 0.0, 1_000.0))
        .expect("portfolio baseline should publish");
    feed.on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_100, -20.0, -20.0, 960.0))
        .expect("portfolio drawdown should publish");
    let preserved_drawdown = feed
        .on_portfolio_snapshot(&portfolio_snapshot(account_id, 1_200, -20.0, -20.0, 950.0))
        .expect("flat daily pnl with lower equity should publish");

    assert_eq!(
        preserved_drawdown.current_equity,
        Some(Decimal::new(950, 0))
    );
    assert_eq!(preserved_drawdown.peak_equity, Some(Decimal::new(1_000, 0)));
    let error = admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 1_250)
        .expect_err("lower equity must not erase the drawdown halt");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons.contains(&LossHaltReason::MaxDrawdownLimit)
    ));
}

#[test]
fn feed_fails_closed_on_mixed_currency_portfolio_without_base_currency() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-006");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
            active_position_pnl_max_entries: 64,
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-007");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
            active_position_pnl_max_entries: 64,
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-008");
    let mut feed = LossGovernorRuntimeFeed::new(
        LossGovernorRuntimeFeedConfig {
            account_id,
            rolling_window_ns: 250,
            active_position_pnl_max_entries: 64,
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.recorder(),
        loss_policy(),
    ));
    let account_id = AccountId::from("SIM-LOSS-009");

    let invocations: Rc<RefCell<Vec<(u64, Option<Decimal>)>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = invocations.clone();
    let handler: LossGovernorHaltActionHandler = Rc::new(
        move |snapshot: Option<&LossSnapshot>, observed_at_ns: u64, _| {
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
            active_position_pnl_max_entries: 64,
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
        max_daily_loss: Some(Decimal::new(25, 0)),
        max_rolling_loss: Some(Decimal::new(30, 0)),
        max_drawdown: Some(Decimal::new(40, 0)),
    }
}

fn loss_snapshot_at(observed_at_ns: u64) -> LossSnapshot {
    LossSnapshot {
        source: "nt_loss_runtime_feed".to_string(),
        observed_at_ns,
        per_trade_pnl: Some(Decimal::ZERO),
        daily_pnl: Some(Decimal::ZERO),
        rolling_pnl: Some(Decimal::ZERO),
        current_equity: Some(Decimal::new(1_000, 0)),
        peak_equity: Some(Decimal::new(1_000, 0)),
        source_observations: LossSourceObservationTimestamps::unobserved(),
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
        risk_reducing_exit_proof: None,
        kill_switch_forced_reduction: None,
        admission_evidence: None,
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
        None,
        false,
        vec![],
        vec![],
        vec![],
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
        None,
        false,
        vec![],
        vec![],
        vec![],
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
        None,
        false,
        vec![],
        vec![],
        vec![],
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

fn opened_position_event(
    account_id: AccountId,
    position_id: &str,
    instrument_id: &str,
    ts_event: u64,
) -> PositionEvent {
    PositionEvent::PositionOpened(PositionOpened {
        trader_id: TraderId::from("TRADER-LOSS-001"),
        strategy_id: StrategyId::from("STRATEGY-LOSS-001"),
        instrument_id: InstrumentId::from(instrument_id),
        position_id: PositionId::from(position_id),
        account_id,
        opening_order_id: ClientOrderId::from("ORDER-LOSS-OPENED"),
        entry: OrderSide::Buy,
        side: PositionSide::Long,
        signed_qty: 1.0,
        quantity: Quantity::from("1"),
        last_qty: Quantity::from("1"),
        last_px: Price::from("1.00"),
        currency: Currency::USD(),
        avg_px_open: 1.0,
        event_id: UUID4::default(),
        ts_event: UnixNanos::from(ts_event),
        ts_init: UnixNanos::from(ts_event),
    })
}

fn closed_position_event(
    account_id: AccountId,
    position_id: &str,
    instrument_id: &str,
    ts_event: u64,
    realized_pnl: f64,
) -> PositionEvent {
    PositionEvent::PositionClosed(PositionClosed {
        trader_id: TraderId::from("TRADER-LOSS-001"),
        strategy_id: StrategyId::from("STRATEGY-LOSS-001"),
        instrument_id: InstrumentId::from(instrument_id),
        position_id: PositionId::from(position_id),
        account_id,
        opening_order_id: ClientOrderId::from("ORDER-LOSS-OPENED"),
        closing_order_id: Some(ClientOrderId::from("ORDER-LOSS-CLOSED")),
        entry: OrderSide::Buy,
        side: PositionSide::Long,
        signed_qty: 0.0,
        quantity: Quantity::from("0"),
        peak_quantity: Quantity::from("1"),
        last_qty: Quantity::from("1"),
        last_px: Price::from("1.00"),
        currency: Currency::USD(),
        avg_px_open: 1.0,
        avg_px_close: Some(1.0),
        realized_return: 0.0,
        realized_pnl: Some(Money::new(realized_pnl, Currency::USD())),
        unrealized_pnl: Money::new(0.0, Currency::USD()),
        duration: DurationNanos::from(1_u64),
        event_id: UUID4::default(),
        ts_opened: UnixNanos::from(1),
        ts_closed: Some(UnixNanos::from(ts_event)),
        ts_event: UnixNanos::from(ts_event),
        ts_init: UnixNanos::from(ts_event),
    })
}

fn changed_position_event(
    account_id: AccountId,
    ts_event: u64,
    unrealized_pnl: f64,
) -> PositionEvent {
    changed_position_event_for_position(
        account_id,
        "POSITION-LOSS-001",
        "INSTRUMENT-LOSS-001.SIM",
        ts_event,
        unrealized_pnl,
    )
}

fn changed_position_event_for_position(
    account_id: AccountId,
    position_id: &str,
    instrument_id: &str,
    ts_event: u64,
    unrealized_pnl: f64,
) -> PositionEvent {
    PositionEvent::PositionChanged(PositionChanged {
        trader_id: TraderId::from("TRADER-LOSS-001"),
        strategy_id: StrategyId::from("STRATEGY-LOSS-001"),
        instrument_id: InstrumentId::from(instrument_id),
        position_id: PositionId::from(position_id),
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
        None,
        false,
        vec![],
        vec![],
        vec![],
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
        None,
        false,
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
