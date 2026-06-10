mod support;

use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};

use bolt_v2::{
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState, KillSwitchStateKind},
    bolt_v3_kill_switch_store::{
        KillSwitchRecoveryReason, KillSwitchRecoveryState, KillSwitchStore,
    },
    bolt_v3_loss_protection::{
        KillSwitchLossAction, KillSwitchLossActionKind, KillSwitchLossActionSink,
        KillSwitchLossProtection, KillSwitchLossProtectionConfig, RealizedPnlObservation,
        seed_admission_from_kill_switch_store,
    },
    bolt_v3_submit_admission::{
        BoltV3KillSwitchForcedReductionPolicy, BoltV3SubmitAdmissionError,
        BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
        BoltV3SubmitLifecyclePolicy,
    },
};
use nautilus_core::UUID4;
use nautilus_model::{
    enums::{OrderSide, PositionAdjustmentType},
    events::{PositionAdjusted, PositionEvent},
    identifiers::{AccountId, InstrumentId, PositionId, StrategyId, TraderId},
    types::{Currency, Money},
};
use rust_decimal::Decimal;

#[test]
fn realized_loss_breach_latches_persists_and_emits_flatten_actions() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-breach");
    let store = KillSwitchStore::new(temp.path().join("kill-switch.json"));
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    )));
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
        })
        .expect("first loss should record below the limit");
    let latched = protection
        .record_realized_pnl(RealizedPnlObservation {
            source: "nt_position_event",
            observed_at_unix_nanos: 1_717_200_000_000_000_100,
            realized_pnl: Decimal::new(-26, 0),
        })
        .expect("second loss should breach the daily limit")
        .expect("breach should return the latched state");

    assert!(matches!(latched, KillSwitchState::Halting { .. }));
    assert!(matches!(
        admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::Halting
        })
    ));
    assert!(matches!(
        protection
            .store()
            .load_recovery_state()
            .expect("persisted halt should be readable"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::UnresolvedHalt,
            state: Some(KillSwitchState::Halting { .. })
        }
    ));

    let recorded = actions.actions();
    assert_eq!(
        recorded
            .iter()
            .map(|action| action.kind)
            .collect::<Vec<_>>(),
        vec![
            KillSwitchLossActionKind::CancelOpenOrders,
            KillSwitchLossActionKind::FlattenPositions,
        ]
    );
    assert!(
        recorded
            .iter()
            .all(|action| action.halt_id == latched.halt_id())
    );
    assert!(
        recorded
            .iter()
            .all(|action| action.policy_sha256 == loss_config(Decimal::new(50, 0)).policy_sha256)
    );
}

#[test]
fn startup_recovery_blocks_entries_from_halting_and_missing_store_files() {
    let temp = support::TempCaseDir::new("bolt-v3-loss-protection-restart");
    let halting_store = KillSwitchStore::new(temp.path().join("halting.json"));
    let halting = KillSwitchState::Halting {
        halt_id: "halt-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "nt_position_event",
            1_717_200_000_000_000_000,
            "daily_realized_loss_limit",
        ),
    };
    halting_store
        .write_state(&halting)
        .expect("halting state should persist");

    let admission = Arc::new(BoltV3SubmitAdmissionState::new(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    )));
    let recovered = seed_admission_from_kill_switch_store(&admission, &halting_store)
        .expect("halting recovery should seed admission");

    assert_eq!(recovered, halting);
    assert!(matches!(
        admission.admit(&entry_request()),
        Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
            state: KillSwitchStateKind::Halting
        })
    ));

    let missing_store = KillSwitchStore::new(temp.path().join("missing.json"));
    let missing_admission = Arc::new(BoltV3SubmitAdmissionState::new(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    )));
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
    let store = KillSwitchStore::new(temp.path().join("kill-switch.json"));
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    )));
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

    assert!(matches!(latched, KillSwitchState::Halting { .. }));
    assert_eq!(actions.actions().len(), 2);
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

fn loss_config(daily_realized_loss_limit: Decimal) -> KillSwitchLossProtectionConfig {
    KillSwitchLossProtectionConfig {
        daily_realized_loss_limit,
        forced_reduction_policy: BoltV3KillSwitchForcedReductionPolicy::new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4,
            Decimal::new(100, 0),
        )
        .expect("forced-reduction policy should be valid"),
        policy_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
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
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_proof: None,
        kill_switch_forced_reduction: None,
    }
}

fn adjusted_position_event(
    account_id: &str,
    instrument_id: &str,
    pnl: f64,
    ts_event: u64,
) -> PositionEvent {
    PositionEvent::PositionAdjusted(PositionAdjusted::new(
        TraderId::from("TESTER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from(instrument_id),
        PositionId::from("P-001"),
        AccountId::from(account_id),
        PositionAdjustmentType::Commission,
        None,
        Some(Money::new(pnl, Currency::USDC())),
        None,
        UUID4::default(),
        ts_event.into(),
        ts_event.into(),
    ))
}

trait HaltIdForTest {
    fn halt_id(&self) -> &str;
}

impl HaltIdForTest for KillSwitchState {
    fn halt_id(&self) -> &str {
        match self {
            KillSwitchState::Halting { halt_id, .. }
            | KillSwitchState::Halted { halt_id, .. }
            | KillSwitchState::Flat { halt_id }
            | KillSwitchState::FailedManualIntervention { halt_id, .. } => halt_id,
            KillSwitchState::Armed => "",
        }
    }
}
