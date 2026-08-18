use crate::support;

use std::{
    fs,
    rc::Rc,
    sync::{Arc, Mutex},
};

use bolt_v2::{
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState, KillSwitchStateKind},
    bolt_v3_kill_switch_store::{
        KillSwitchPendingHaltActionsSnapshot, KillSwitchRecoveryReason, KillSwitchRecoveryState,
        KillSwitchStore,
    },
    bolt_v3_loss_protection::{
        KillSwitchLossAction, KillSwitchLossActionKind, KillSwitchLossActionSink,
        KillSwitchLossProtection, KillSwitchLossProtectionConfig, PositionRealizedPnlObservation,
        RealizedPnlObservation, seed_admission_from_kill_switch_store,
    },
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
        BoltV3SubmitIntentKind,
    },
};
use nautilus_core::UUID4;
use nautilus_model::{
    enums::{OrderSide, PositionAdjustmentType, PositionSide},
    events::{PositionAdjusted, PositionChanged, PositionEvent},
    identifiers::{AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId, TraderId},
    types::{Currency, Money, Price, Quantity},
};
use rust_decimal::Decimal;

const TEST_MAX_STATE_FILE_BYTES: u64 = 65_536;
const TEST_ACTION_RETRY_INTERVAL_MS: u64 = 250;
const TEST_ACTION_RETRY_TIMEOUT_MS: u64 = 5_000;
const NANOS_PER_MILLISECOND: u64 = 1_000_000;
const NANOS_PER_UTC_DAY: u64 = 86_400_000_000_000;

#[test]
fn realized_loss_breach_latches_persists_and_emits_flatten_actions() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-breach");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(50, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: 1_717_200_000_000_000_000,
            realized_pnl: Decimal::new(-25, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect("first loss should record below the limit");
    let latched = protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: 1_717_200_000_000_000_100,
            realized_pnl: Decimal::new(-26, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect("second loss should breach the daily limit")
        .expect("breach should return the latched state");

    assert!(matches!(latched, KillSwitchState::Halted { .. }));
    assert!(matches!(
        admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::Halted
        })
    ));
    assert!(matches!(
        protection
            .store()
            .load_recovery_state()
            .expect("persisted halt should be readable"),
        KillSwitchRecoveryState::Recovered(KillSwitchState::Halted { .. })
    ));

    let recorded = actions.actions();
    assert_eq!(
        recorded
            .iter()
            .map(|action| action.kind)
            .collect::<Vec<_>>(),
        vec![KillSwitchLossActionKind::FlattenPositions]
    );
    assert!(
        recorded
            .iter()
            .all(|action| action.halt_id == latched.halt_id())
    );
}

#[test]
fn startup_recovery_blocks_entries_from_halting_and_missing_store_files() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-restart");
    let halting_store =
        KillSwitchStore::new(temp.path().join("halting.json"), TEST_MAX_STATE_FILE_BYTES);
    let halting = KillSwitchState::Halting {
        halt_id: "halt-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "nt_position_event",
            1_717_200_000_000_000_000,
            "max_utc_daily_realized_loss",
        ),
    };
    halting_store
        .write_state(&halting)
        .expect("halting state should persist");

    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let recovered = seed_admission_from_kill_switch_store(&admission, &halting_store)
        .expect("halting recovery should seed admission");

    assert_eq!(recovered, halting);
    assert!(matches!(
        admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::Halting
        })
    ));

    let missing_store =
        KillSwitchStore::new(temp.path().join("missing.json"), TEST_MAX_STATE_FILE_BYTES);
    let missing_admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let recovered_missing =
        seed_admission_from_kill_switch_store(&missing_admission, &missing_store)
            .expect("missing store should fail closed into admission state");

    assert!(matches!(
        recovered_missing,
        KillSwitchState::FailedManualIntervention { .. }
    ));
    assert!(matches!(
        missing_admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::FailedManualIntervention
        })
    ));
}

#[test]
fn position_event_filter_requires_configured_account_and_instrument() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-position-filter");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(1, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    assert!(
        protection
            .record_position_event(&adjusted_position_event(
                "POLYMARKET-001",
                "ETH-USD.BINANCE",
                -2.0,
                1_717_200_000_000_000_000,
            ))
            .expect("unconfigured instrument event should be handled")
            .is_none()
    );
    assert!(actions.actions().is_empty());
    assert!(admission.admit(&entry_request()).is_ok());

    let latched = protection
        .record_position_event(&adjusted_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -2.0,
            1_717_200_000_000_000_001,
        ))
        .expect("configured instrument event should be handled")
        .expect("configured instrument event should breach");

    assert!(matches!(latched, KillSwitchState::Halted { .. }));
    assert_eq!(actions.actions().len(), 1);
}

#[test]
fn mixed_settlement_currency_realized_pnl_fails_closed() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-mixed-currency");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    // A high daily limit so neither single realized loss can breach on its own:
    // this isolates the mixed-currency integrity guard from the loss-breach path.
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(1_000, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    // First realized loss settled in USDC, comfortably below the daily limit.
    assert!(
        protection
            .record_position_event(&changed_position_event_with_currency(
                "POLYMARKET-001",
                "BTC-USD.BINANCE",
                -10.0,
                Currency::USDC(),
                1_717_200_000_000_000_000,
            ))
            .expect("first settlement-currency event should record")
            .is_none()
    );
    assert!(admission.admit(&entry_request()).is_ok());

    // A second realized loss settled in a DIFFERENT currency cannot be combined
    // into one daily realized-loss figure. Summing raw decimals across currencies
    // would let the kill switch halt early or — worse — fail to halt, so the
    // accumulator must fail closed instead of silently mixing currencies
    // (mirrors the mixed-currency handling in bolt_v3_loss_runtime_feed).
    let error = protection
        .record_position_event(&changed_position_event_with_currency(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -10.0,
            Currency::USD(),
            1_717_200_000_000_000_100,
        ))
        .expect_err("mixed settlement currency must fail closed");

    assert!(
        error.to_string().contains("mixed_settlement_currency"),
        "error should name the settlement-currency integrity failure: {error}"
    );
    // Admission is latched into FailedManualIntervention: an integrity failure is
    // not a recoverable loss breach, it requires manual operator intervention.
    let mut post_failure_request = entry_request();
    post_failure_request.client_order_id = "client-order-2".to_string();
    assert!(matches!(
        admission.admit(&post_failure_request),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::FailedManualIntervention
        })
    ));
    // No flatten action is emitted: this path fails closed via the latch, it does
    // not run the loss-breach halt-action dispatch.
    assert!(actions.actions().is_empty());
}

#[test]
fn persistence_failure_fails_closed_before_returning_error() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-persist-failure");
    let parent_path = temp.path().join("state");
    fs::write(&parent_path, "not-a-directory").expect("blocking path should write");
    let store = KillSwitchStore::new(
        parent_path.join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(1, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    let error = protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: 1_717_200_000_000_000_000,
            realized_pnl: Decimal::new(-2, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect_err("failed persistence should return an error");

    assert!(
        error
            .to_string()
            .contains("daily realized loss halt persistence failed")
    );
    assert!(matches!(
        admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::FailedManualIntervention
        })
    ));
    assert!(actions.actions().is_empty());
}

#[test]
fn cumulative_position_pnl_keeps_prior_baseline_across_utc_day_rollover() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-cumulative-rollover");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(12, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    assert!(
        protection
            .record_position_event(&changed_position_event(
                "POLYMARKET-001",
                "BTC-USD.BINANCE",
                -10.0,
                NANOS_PER_UTC_DAY - 1,
            ))
            .expect("prior-day cumulative event should record")
            .is_none()
    );
    assert!(
        protection
            .record_position_event(&changed_position_event(
                "POLYMARKET-001",
                "BTC-USD.BINANCE",
                -15.0,
                NANOS_PER_UTC_DAY,
            ))
            .expect("first current-day cumulative event should record")
            .is_none()
    );
    assert!(
        protection
            .record_position_event(&changed_position_event(
                "POLYMARKET-001",
                "BTC-USD.BINANCE",
                -20.0,
                NANOS_PER_UTC_DAY + 1,
            ))
            .expect("second current-day cumulative event should not phantom breach")
            .is_none()
    );

    assert!(admission.admit(&entry_request()).is_ok());
    assert!(actions.actions().is_empty());
}

#[test]
fn record_realized_pnl_resets_accumulator_on_utc_day_rollover() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-day-reset");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(50, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    // Day 1: a -40 realized loss is under the 50 limit -> no breach.
    assert!(
        protection
            .record_realized_pnl(RealizedPnlObservation {
                source: "nt_position_event",
                observed_at_unix_nanos: NANOS_PER_UTC_DAY,
                realized_pnl: Decimal::new(-40, 0),
                settlement_currency: Currency::USDC(),
            })
            .expect("day-1 loss should record below the limit")
            .is_none()
    );

    // Day 2: another -40. Without a fresh-day reset the running total would be
    // -80 and breach the 50 limit; the UTC forward-day reset keeps the current
    // day at -40 (no breach), proving the reset fires.
    assert!(
        protection
            .record_realized_pnl(RealizedPnlObservation {
                source: "nt_position_event",
                observed_at_unix_nanos: NANOS_PER_UTC_DAY * 2,
                realized_pnl: Decimal::new(-40, 0),
                settlement_currency: Currency::USDC(),
            })
            .expect("day-2 loss should reset the accumulator and stay below the limit")
            .is_none()
    );

    // Still day 2: a further -5 brings the current day to -45 (< 50, no breach).
    // Were the prior day's -40 still counted the total would be -85 and breach,
    // so this proves only current-day losses accumulate after the rollover.
    assert!(
        protection
            .record_realized_pnl(RealizedPnlObservation {
                source: "nt_position_event",
                observed_at_unix_nanos: NANOS_PER_UTC_DAY * 2 + 1,
                realized_pnl: Decimal::new(-5, 0),
                settlement_currency: Currency::USDC(),
            })
            .expect("second day-2 loss should not phantom-breach from the prior day")
            .is_none()
    );

    assert!(admission.admit(&entry_request()).is_ok());
    assert!(actions.actions().is_empty());
}

#[test]
fn failed_halt_actions_retry_after_configured_interval_until_success() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-action-retry");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(FlakyLossActionSink::new(1));
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(1, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: 1_717_200_000_000_000_000,
            realized_pnl: Decimal::new(-2, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect_err("first flatten dispatch should fail");
    assert_eq!(actions.flatten_attempts(), 1);
    assert!(matches!(
        admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::Halting
        })
    ));

    protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: 1_717_200_000_000_000_000
                + (TEST_ACTION_RETRY_INTERVAL_MS * NANOS_PER_MILLISECOND),
            realized_pnl: Decimal::ZERO,
            settlement_currency: Currency::USDC(),
        })
        .expect("configured action retry should succeed");

    assert_eq!(actions.flatten_attempts(), 2);
    assert!(matches!(
        admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::Halted
        })
    ));
    assert!(matches!(
        protection
            .store()
            .load_recovery_state()
            .expect("retry-success state should be durable"),
        KillSwitchRecoveryState::Recovered(KillSwitchState::Halted { .. })
    ));
}

#[test]
fn daily_realized_pnl_survives_restart_until_utc_bucket_rolls_forward() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-daily-persist");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut first = KillSwitchLossProtection::new(
        loss_config(Decimal::new(50, 0)),
        admission,
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    first
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: 1_717_200_000_000_000_000,
            realized_pnl: Decimal::new(-40, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect("below-limit loss should persist the runtime accumulator");

    let restart_admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let restart_store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let mut restarted = KillSwitchLossProtection::new(
        loss_config(Decimal::new(50, 0)),
        restart_admission.clone(),
        restart_store,
        actions.clone(),
    )
    .expect("loss protection should initialize after restart");

    assert_eq!(
        restarted
            .seed_from_store(1_717_200_000_000_000_000)
            .expect("restart should recover armed loss snapshot"),
        KillSwitchState::Armed
    );
    let latched = restarted
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: 1_717_200_000_000_000_100,
            realized_pnl: Decimal::new(-15, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect("post-restart loss should breach with persisted daily total")
        .expect("persisted daily total should latch the kill switch");

    assert!(matches!(latched, KillSwitchState::Halted { .. }));
    assert!(matches!(
        restart_admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::Halted
        })
    ));
}

#[test]
fn recovered_armed_loss_snapshot_rechecks_lowered_daily_limit() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-restart-lowered-limit");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut first = KillSwitchLossProtection::new(
        loss_config(Decimal::new(100, 0)),
        admission,
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    first
        .record_position_event(&changed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -75.0,
            1_717_200_000_000_000_000,
        ))
        .expect("below old limit loss should persist an armed snapshot");

    let restart_admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let restart_store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let mut restarted = KillSwitchLossProtection::new(
        loss_config(Decimal::new(50, 0)),
        restart_admission.clone(),
        restart_store,
        actions.clone(),
    )
    .expect("loss protection should initialize with lowered limit");

    let recovered = restarted
        .seed_from_store(1_717_200_000_000_000_100)
        .expect("restart should recover and re-evaluate the lowered limit");

    assert!(matches!(recovered, KillSwitchState::Halted { .. }));
    assert!(matches!(
        restart_admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::Halted
        })
    ));
}

#[test]
fn cumulative_position_baseline_survives_restart_without_current_day_false_positive() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-cumulative-persist");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut first = KillSwitchLossProtection::new(
        loss_config(Decimal::new(50, 0)),
        admission,
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    first
        .record_position_event(&changed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -40.0,
            NANOS_PER_UTC_DAY - 1,
        ))
        .expect("prior-day cumulative event should persist the baseline");

    let restart_admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let restart_store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let mut restarted = KillSwitchLossProtection::new(
        loss_config(Decimal::new(50, 0)),
        restart_admission.clone(),
        restart_store,
        actions.clone(),
    )
    .expect("loss protection should initialize after restart");
    restarted
        .seed_from_store(NANOS_PER_UTC_DAY)
        .expect("restart should recover cumulative baseline");

    assert!(
        restarted
            .record_position_event(&changed_position_event(
                "POLYMARKET-001",
                "BTC-USD.BINANCE",
                -60.0,
                NANOS_PER_UTC_DAY,
            ))
            .expect("first current-day cumulative event should use restored baseline")
            .is_none()
    );
    assert!(restart_admission.admit(&entry_request()).is_ok());
    assert!(actions.actions().is_empty());
}

#[test]
fn stale_utc_bucket_events_do_not_clear_current_day_loss_accumulator() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-stale-bucket");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(50, 0)),
        admission,
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: NANOS_PER_UTC_DAY * 2,
            realized_pnl: Decimal::new(-40, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect("current-day loss should record");
    protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: NANOS_PER_UTC_DAY,
            realized_pnl: Decimal::ZERO,
            settlement_currency: Currency::USDC(),
        })
        .expect("stale prior-day event should not reset current-day loss");

    let latched = protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: NANOS_PER_UTC_DAY * 2 + 1,
            realized_pnl: Decimal::new(-15, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect("current-day loss should still breach after stale event")
        .expect("current-day accumulator should not have been cleared");

    assert!(matches!(latched, KillSwitchState::Halted { .. }));
}

#[test]
fn utc_day_rollover_prunes_completed_position_dedup_snapshots() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-rollover-prunes-dedup");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection =
        KillSwitchLossProtection::new(loss_config(Decimal::new(100, 0)), admission, store, actions)
            .expect("loss protection should initialize");

    protection
        .record_position_event(&adjusted_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -5.0,
            1,
        ))
        .expect("adjustment should record");
    protection
        .record_position_event(&changed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -10.0,
            2,
        ))
        .expect("open position cumulative pnl should record");
    protection
        .record_position_event(&closed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -15.0,
            3,
        ))
        .expect("closed position cumulative pnl should record");

    let before_rollover = protection
        .store()
        .load_recovery_record()
        .expect("snapshot before rollover should load")
        .loss_protection
        .expect("snapshot before rollover should exist");
    assert_eq!(before_rollover.adjusted_position_pnl.len(), 1);
    assert_eq!(before_rollover.closed_position_pnl.len(), 1);

    protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: NANOS_PER_UTC_DAY,
            realized_pnl: Decimal::ZERO,
            settlement_currency: Currency::USDC(),
        })
        .expect("first next-day observation should roll the bucket forward");

    protection
        .record_position_event(&adjusted_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -5.0,
            1,
        ))
        .expect("stale prior-day adjustment should be ignored after rollover");

    let after_rollover = protection
        .store()
        .load_recovery_record()
        .expect("snapshot after rollover should load")
        .loss_protection
        .expect("snapshot after rollover should exist");
    assert!(after_rollover.adjusted_position_pnl.is_empty());
    assert!(after_rollover.closed_position_pnl.is_empty());
}

#[test]
fn closed_position_prunes_cumulative_baseline_before_position_id_reuse() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-prune-closed");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(30, 0)),
        admission,
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    protection
        .record_position_event(&changed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -20.0,
            1,
        ))
        .expect("open position cumulative pnl should record");
    protection
        .record_position_event(&closed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -25.0,
            2,
        ))
        .expect("closed position cumulative pnl should record and prune baseline");

    let latched = protection
        .record_position_event(&changed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -10.0,
            3,
        ))
        .expect("reused position id should start a fresh cumulative baseline")
        .expect("fresh reused position loss should breach the daily limit");

    assert!(matches!(latched, KillSwitchState::Halted { .. }));
}

#[test]
fn duplicate_closed_position_event_still_prunes_cumulative_baseline() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-prune-duplicate-close");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(25, 0)),
        admission,
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    protection
        .record_position_event(&changed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -20.0,
            1,
        ))
        .expect("open position cumulative pnl should record");
    protection
        .record_position_event(&closed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -20.0,
            1,
        ))
        .expect("duplicate close should prune the cumulative baseline");

    let latched = protection
        .record_position_event(&changed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -10.0,
            2,
        ))
        .expect("reused position id should not inherit the closed baseline")
        .expect("fresh reused position loss should breach the daily limit");

    assert!(matches!(latched, KillSwitchState::Halted { .. }));
}

#[test]
fn same_timestamp_reopen_with_distinct_pnl_counts_fresh_cycle() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-reopen-same-ts");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(25, 0)),
        admission,
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    protection
        .record_position_event(&changed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -20.0,
            10,
        ))
        .expect("initial cumulative pnl should record");
    protection
        .record_position_event(&closed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -20.0,
            10,
        ))
        .expect("same-timestamp duplicate close should prune cumulative baseline");

    let latched = protection
        .record_position_event(&changed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -10.0,
            10,
        ))
        .expect("same-timestamp reopen with distinct pnl should be fresh")
        .expect("fresh cycle should count toward the daily limit");

    assert!(matches!(latched, KillSwitchState::Halted { .. }));
}

#[test]
fn pending_halt_actions_retry_from_timer_without_new_position_events() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-timer-retry");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(FlakyLossActionSink::new(1));
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(1, 0)),
        admission,
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");
    let breach_time = 1_717_200_000_000_000_000;

    protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: breach_time,
            realized_pnl: Decimal::new(-2, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect_err("first flatten dispatch should fail");
    assert_eq!(actions.flatten_attempts(), 1);

    protection
        .poll_pending_halt_actions(
            breach_time + (TEST_ACTION_RETRY_INTERVAL_MS * NANOS_PER_MILLISECOND),
        )
        .expect("timer-driven retry should succeed without a position event");

    assert_eq!(actions.flatten_attempts(), 2);
}

#[test]
fn recovered_pending_halt_actions_retry_from_persisted_schedule() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-recovered-pending-retry");
    let path = temp.path().join("kill-switch.json");
    let store = KillSwitchStore::new(path.clone(), TEST_MAX_STATE_FILE_BYTES);
    let pending_time = 1_717_200_000_000_000_000;
    let halting = KillSwitchState::Halting {
        halt_id: "halt-recovered-pending".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "nt_position_event",
            pending_time,
            "max_utc_daily_realized_loss",
        ),
    };
    let mut snapshot = bolt_v2::bolt_v3_kill_switch_store::initial_armed_loss_protection_snapshot();
    snapshot.pending_halt_actions = Some(KillSwitchPendingHaltActionsSnapshot {
        next_retry_at_unix_nanos: pending_time + NANOS_PER_MILLISECOND,
        retry_deadline_unix_nanos: pending_time
            + ((TEST_ACTION_RETRY_TIMEOUT_MS + 1) * NANOS_PER_MILLISECOND),
    });
    store
        .write_state_with_loss_snapshot(&halting, Some(&snapshot))
        .expect("halting state with pending halt actions should persist");

    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut restarted = KillSwitchLossProtection::new(
        loss_config(Decimal::new(1, 0)),
        admission,
        KillSwitchStore::new(path, TEST_MAX_STATE_FILE_BYTES),
        actions.clone(),
    )
    .expect("loss protection should initialize");

    let recovered = restarted
        .seed_from_store(pending_time)
        .expect("restart should recover pending halt-action schedule");
    assert!(matches!(recovered, KillSwitchState::Halting { .. }));
    assert!(actions.actions().is_empty());

    restarted
        .poll_pending_halt_actions(pending_time)
        .expect("timer before next retry should not dispatch");
    assert!(actions.actions().is_empty());

    restarted
        .poll_pending_halt_actions(pending_time + NANOS_PER_MILLISECOND)
        .expect("persisted retry schedule should dispatch at next retry");

    assert_eq!(actions.actions().len(), 1);
    assert!(matches!(
        restarted
            .store()
            .load_recovery_state()
            .expect("recovered pending retry should persist halted state"),
        KillSwitchRecoveryState::Recovered(KillSwitchState::Halted { .. })
    ));
}

#[test]
fn pending_halt_action_timeout_persists_failed_manual_intervention() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-timer-timeout");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(FlakyLossActionSink::new(usize::MAX));
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(1, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");
    let breach_time = 1_717_200_000_000_000_000;

    protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: breach_time,
            realized_pnl: Decimal::new(-2, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect_err("first flatten dispatch should fail");
    protection
        .poll_pending_halt_actions(
            breach_time + ((TEST_ACTION_RETRY_TIMEOUT_MS + 1) * NANOS_PER_MILLISECOND),
        )
        .expect_err("timer-driven timeout should fail closed");

    assert!(matches!(
        admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::FailedManualIntervention
        })
    ));
    assert!(matches!(
        protection
            .store()
            .load_recovery_state()
            .expect("failed state should be durable"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::UnresolvedHalt,
            state: Some(KillSwitchState::FailedManualIntervention { .. })
        }
    ));
}

#[test]
fn halt_persistence_failure_invalidates_preexisting_permissive_store() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-poison-armed-store");
    let path = temp.path().join("kill-switch.json");
    KillSwitchStore::new(path.clone(), TEST_MAX_STATE_FILE_BYTES)
        .write_state(&KillSwitchState::Armed)
        .expect("preexisting permissive state should persist");
    let constrained_store = KillSwitchStore::new(path, 96);
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(1, 0)),
        admission,
        constrained_store,
        actions,
    )
    .expect("loss protection should initialize");

    protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: 1_717_200_000_000_000_000,
            realized_pnl: Decimal::new(-2, 0),
            settlement_currency: Currency::USDC(),
        })
        .expect_err("oversized halt evidence should fail persistence");

    assert!(matches!(
        protection
            .store()
            .load_recovery_state()
            .expect("store should no longer recover Armed"),
        KillSwitchRecoveryState::FailClosed { .. }
    ));
}

#[test]
fn duplicate_closed_position_replay_after_prune_counts_once() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-duplicate-close-replay");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(30, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    protection
        .record_position_event(&changed_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -20.0,
            1,
        ))
        .expect("open position cumulative pnl should record");
    assert!(
        protection
            .record_position_event(&closed_position_event(
                "POLYMARKET-001",
                "BTC-USD.BINANCE",
                -25.0,
                3,
            ))
            .expect("first close should add only the close delta")
            .is_none()
    );

    assert!(
        protection
            .record_position_event(&closed_position_event(
                "POLYMARKET-001",
                "BTC-USD.BINANCE",
                -25.0,
                2,
            ))
            .expect("duplicate close replay should be ignored")
            .is_none()
    );
    assert!(admission.admit(&entry_request()).is_ok());
    assert!(actions.actions().is_empty());
}

#[test]
fn duplicate_adjusted_position_replay_counts_delta_once() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-duplicate-adjusted-replay");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(15, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    let duplicate_event_id = UUID4::new();
    protection
        .record_position_event(&adjusted_position_event_with_id(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -10.0,
            1,
            duplicate_event_id,
        ))
        .expect("first adjustment should record below limit");
    protection
        .record_position_event(&adjusted_position_event_with_id(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -10.0,
            1,
            duplicate_event_id,
        ))
        .expect("duplicate adjustment should not double count");
    assert!(admission.admit(&entry_request()).is_ok());

    let latched = protection
        .record_position_event(&adjusted_position_event(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -6.0,
            2,
        ))
        .expect("new adjustment should be counted")
        .expect("total unique adjustments should breach");

    assert!(matches!(latched, KillSwitchState::Halted { .. }));
    assert_eq!(actions.actions().len(), 1);
}

#[test]
fn settlement_position_pnl_dedupes_by_settlement_key_before_daily_accumulation() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-settlement-key-dedupe");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(10, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    protection
        .record_position_realized_pnl(settlement_pnl_observation(
            "MKT-1:P-SETTLED",
            "P-SETTLED",
            Decimal::new(-6, 0),
            1_717_200_000_000_000_000,
        ))
        .expect("first settlement should record below limit");
    protection
        .record_position_realized_pnl(settlement_pnl_observation(
            "MKT-1:P-SETTLED",
            "P-SETTLED",
            Decimal::new(-6, 0),
            1_717_200_000_000_000_100,
        ))
        .expect("duplicate settlement key should not double count");
    assert!(admission.admit(&entry_request()).is_ok());
    assert!(actions.actions().is_empty());

    let latched = protection
        .record_position_realized_pnl(settlement_pnl_observation(
            "MKT-1:P-SETTLED-2",
            "P-SETTLED-2",
            Decimal::new(-5, 0),
            1_717_200_000_000_000_200,
        ))
        .expect("distinct settlement key should be counted")
        .expect("unique settlement losses should breach the daily limit");

    assert!(matches!(latched, KillSwitchState::Halted { .. }));
    assert_eq!(actions.actions().len(), 1);
}

#[test]
fn mixed_settlement_currency_adjusted_pnl_fails_closed() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-mixed-adjusted-currency");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(1_000, 0)),
        admission.clone(),
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    protection
        .record_position_event(&changed_position_event_with_currency(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -10.0,
            Currency::USDC(),
            1_717_200_000_000_000_000,
        ))
        .expect("first currency should establish the accumulator currency");

    let error = protection
        .record_position_event(&adjusted_position_event_with_currency(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -5.0,
            Currency::USD(),
            1_717_200_000_000_000_100,
        ))
        .expect_err("adjusted pnl in another currency must fail closed");

    assert!(
        error.to_string().contains("mixed_settlement_currency"),
        "error should name the settlement-currency integrity failure: {error}"
    );
    assert!(matches!(
        admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::FailedManualIntervention
        })
    ));
}

#[test]
fn late_distinct_adjusted_position_event_is_counted_by_event_id() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-late-adjusted-event");
    let store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(15, 0)),
        admission,
        store,
        actions.clone(),
    )
    .expect("loss protection should initialize");

    protection
        .record_position_event(&adjusted_position_event_with_id(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -10.0,
            2,
            UUID4::new(),
        ))
        .expect("newer adjustment should record below limit");

    let latched = protection
        .record_position_event(&adjusted_position_event_with_id(
            "POLYMARKET-001",
            "BTC-USD.BINANCE",
            -6.0,
            1,
            UUID4::new(),
        ))
        .expect("late distinct adjustment should still be counted")
        .expect("distinct adjustments should breach even when out of order");

    assert!(matches!(latched, KillSwitchState::Halted { .. }));
    assert_eq!(actions.actions().len(), 1);
}

#[test]
fn halting_recovery_without_pending_snapshot_reissues_flatten() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-halting-recovery-flatten");
    let path = temp.path().join("kill-switch.json");
    let store = KillSwitchStore::new(path.clone(), TEST_MAX_STATE_FILE_BYTES);
    store
        .write_state(&KillSwitchState::Halting {
            halt_id: "halt-1".to_string(),
            trigger: KillSwitchHaltTrigger::loss_governor_breach(
                "nt_position_event",
                1_717_200_000_000_000_000,
                "max_utc_daily_realized_loss",
            ),
        })
        .expect("halting state without loss snapshot should persist");

    let admission = Arc::new(BoltV3SubmitAdmissionState::new(
        support::current_evidence::recording_evidence(),
    ));
    let actions = Rc::new(RecordingLossActionSink::default());
    let mut protection = KillSwitchLossProtection::new(
        loss_config(Decimal::new(50, 0)),
        admission,
        KillSwitchStore::new(path, TEST_MAX_STATE_FILE_BYTES),
        actions.clone(),
    )
    .expect("loss protection should initialize");

    let recovered = protection
        .seed_from_store(1_717_200_000_000_000_000)
        .expect("halting recovery should seed");

    assert!(matches!(recovered, KillSwitchState::Halted { .. }));
    assert_eq!(actions.actions().len(), 1);
    assert!(matches!(
        protection
            .store()
            .load_recovery_state()
            .expect("recovered halt action state should persist"),
        KillSwitchRecoveryState::Recovered(KillSwitchState::Halted { .. })
    ));
}

#[derive(Debug, Default)]
struct RecordingLossActionSink {
    actions: Mutex<Vec<KillSwitchLossAction>>,
}

impl RecordingLossActionSink {
    fn actions(&self) -> Vec<KillSwitchLossAction> {
        self.actions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl KillSwitchLossActionSink for RecordingLossActionSink {
    fn emit(&self, action: KillSwitchLossAction) -> anyhow::Result<()> {
        self.actions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(action);
        Ok(())
    }
}

#[derive(Debug)]
struct FlakyLossActionSink {
    failures_remaining: Mutex<usize>,
    actions: Mutex<Vec<KillSwitchLossAction>>,
}

impl FlakyLossActionSink {
    fn new(flatten_failures: usize) -> Self {
        Self {
            failures_remaining: Mutex::new(flatten_failures),
            actions: Mutex::new(Vec::new()),
        }
    }

    fn flatten_attempts(&self) -> usize {
        self.actions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|action| action.kind == KillSwitchLossActionKind::FlattenPositions)
            .count()
    }
}

impl KillSwitchLossActionSink for FlakyLossActionSink {
    fn emit(&self, action: KillSwitchLossAction) -> anyhow::Result<()> {
        self.actions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(action.clone());
        if action.kind == KillSwitchLossActionKind::FlattenPositions {
            let mut failures = self
                .failures_remaining
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *failures > 0 {
                *failures -= 1;
                return Err(anyhow::anyhow!("configured flatten dispatch failure"));
            }
        }
        Ok(())
    }
}

fn loss_config(max_utc_daily_realized_loss: Decimal) -> KillSwitchLossProtectionConfig {
    KillSwitchLossProtectionConfig {
        max_utc_daily_realized_loss,
        action_retry_interval_ms: TEST_ACTION_RETRY_INTERVAL_MS,
        action_retry_timeout_ms: TEST_ACTION_RETRY_TIMEOUT_MS,
        account_ids: vec!["POLYMARKET-001".to_string()],
        instrument_ids: vec!["BTC-USD.BINANCE".to_string()],
    }
}

fn entry_request() -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        client_order_id: "client-order-1".to_string(),
        instrument_id: "BTC-USD.BINANCE".to_string(),
        notional: Decimal::new(1, 0),
        order_side: OrderSide::Buy,
        order_quantity: Decimal::new(1, 0),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        risk_reducing_exit_proof: None,
        admission_evidence: None,
    }
}

fn settlement_pnl_observation(
    settlement_key: &str,
    position_id: &str,
    realized_pnl: Decimal,
    observed_at_unix_nanos: u64,
) -> PositionRealizedPnlObservation {
    PositionRealizedPnlObservation {
        account_id: "POLYMARKET-001".to_string(),
        instrument_id: "BTC-USD.BINANCE".to_string(),
        position_id: position_id.to_string(),
        event_id: Some(settlement_key.to_string()),
        observed: RealizedPnlObservation {
            source: "settlement",
            observed_at_unix_nanos,
            realized_pnl,
            settlement_currency: Currency::USDC(),
        },
        cumulative_realized_pnl: false,
        closes_position: true,
    }
}

fn adjusted_position_event(
    account_id: &str,
    instrument_id: &str,
    pnl: f64,
    ts_event: u64,
) -> PositionEvent {
    adjusted_position_event_with_id(account_id, instrument_id, pnl, ts_event, UUID4::new())
}

fn adjusted_position_event_with_currency(
    account_id: &str,
    instrument_id: &str,
    pnl: f64,
    settlement_currency: Currency,
    ts_event: u64,
) -> PositionEvent {
    adjusted_position_event_with_id_and_currency(
        account_id,
        instrument_id,
        pnl,
        settlement_currency,
        ts_event,
        UUID4::new(),
    )
}

fn adjusted_position_event_with_id(
    account_id: &str,
    instrument_id: &str,
    pnl: f64,
    ts_event: u64,
    event_id: UUID4,
) -> PositionEvent {
    adjusted_position_event_with_id_and_currency(
        account_id,
        instrument_id,
        pnl,
        Currency::USDC(),
        ts_event,
        event_id,
    )
}

fn adjusted_position_event_with_id_and_currency(
    account_id: &str,
    instrument_id: &str,
    pnl: f64,
    settlement_currency: Currency,
    ts_event: u64,
    event_id: UUID4,
) -> PositionEvent {
    PositionEvent::PositionAdjusted(PositionAdjusted::new(
        TraderId::from("TESTER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from(instrument_id),
        PositionId::from("P-001"),
        AccountId::from(account_id),
        PositionAdjustmentType::Commission,
        None,
        Some(Money::new(pnl, settlement_currency)),
        None,
        event_id,
        ts_event.into(),
        ts_event.into(),
    ))
}

fn changed_position_event(
    account_id: &str,
    instrument_id: &str,
    cumulative_pnl: f64,
    ts_event: u64,
) -> PositionEvent {
    changed_position_event_with_currency(
        account_id,
        instrument_id,
        cumulative_pnl,
        Currency::USDC(),
        ts_event,
    )
}

fn changed_position_event_with_currency(
    account_id: &str,
    instrument_id: &str,
    cumulative_pnl: f64,
    settlement_currency: Currency,
    ts_event: u64,
) -> PositionEvent {
    PositionEvent::PositionChanged(PositionChanged {
        trader_id: TraderId::from("TESTER-001"),
        strategy_id: StrategyId::from("strategy-a"),
        instrument_id: InstrumentId::from(instrument_id),
        position_id: PositionId::from("P-001"),
        account_id: AccountId::from(account_id),
        opening_order_id: ClientOrderId::from("O-19700101-000000-001-001-1"),
        entry: OrderSide::Buy,
        side: PositionSide::Long,
        signed_qty: 1.0,
        quantity: Quantity::from("1"),
        peak_quantity: Quantity::from("1"),
        last_qty: Quantity::from("1"),
        last_px: Price::from("1.0"),
        currency: settlement_currency,
        avg_px_open: 1.0,
        avg_px_close: None,
        realized_return: 0.0,
        realized_pnl: Some(Money::new(cumulative_pnl, settlement_currency)),
        unrealized_pnl: Money::new(0.0, settlement_currency),
        event_id: UUID4::default(),
        ts_opened: ts_event.into(),
        ts_event: ts_event.into(),
        ts_init: ts_event.into(),
    })
}

fn closed_position_event(
    account_id: &str,
    instrument_id: &str,
    cumulative_pnl: f64,
    ts_event: u64,
) -> PositionEvent {
    PositionEvent::PositionClosed(nautilus_model::events::PositionClosed {
        trader_id: TraderId::from("TESTER-001"),
        strategy_id: StrategyId::from("strategy-a"),
        instrument_id: InstrumentId::from(instrument_id),
        position_id: PositionId::from("P-001"),
        account_id: AccountId::from(account_id),
        opening_order_id: ClientOrderId::from("O-19700101-000000-001-001-1"),
        closing_order_id: Some(ClientOrderId::from("O-19700101-000000-001-001-2")),
        entry: OrderSide::Buy,
        side: PositionSide::Long,
        signed_qty: 0.0,
        quantity: Quantity::zero(0),
        peak_quantity: Quantity::from("1"),
        last_qty: Quantity::from("1"),
        last_px: Price::from("1.0"),
        currency: Currency::USDC(),
        avg_px_open: 1.0,
        avg_px_close: Some(1.0),
        realized_return: 0.0,
        realized_pnl: Some(Money::new(cumulative_pnl, Currency::USDC())),
        unrealized_pnl: Money::new(0.0, Currency::USDC()),
        duration: nautilus_core::nanos::DurationNanos::from(1_u64),
        event_id: UUID4::default(),
        ts_opened: 1_u64.into(),
        ts_closed: Some(ts_event.into()),
        ts_event: ts_event.into(),
        ts_init: ts_event.into(),
    })
}

trait HaltIdForTest {
    fn halt_id(&self) -> &str;
}

impl HaltIdForTest for KillSwitchState {
    fn halt_id(&self) -> &str {
        match self {
            KillSwitchState::Halting { halt_id, .. }
            | KillSwitchState::Halted { halt_id, .. }
            | KillSwitchState::Cancelling { halt_id }
            | KillSwitchState::Flattening { halt_id }
            | KillSwitchState::Flat { halt_id }
            | KillSwitchState::FailedManualIntervention { halt_id, .. } => halt_id,
            KillSwitchState::Armed => "",
        }
    }
}
