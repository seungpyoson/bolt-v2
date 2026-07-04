use crate::support;

use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::{
        KillSwitchConfigBlock, LiveSubmitGovernanceBlock, LiveSubmitGovernanceMode,
        load_bolt_v3_config,
    },
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState, KillSwitchStateKind},
    bolt_v3_kill_switch_store::{
        KillSwitchLossProtectionSnapshot, KillSwitchRecoveryState, KillSwitchStore,
    },
    bolt_v3_loss_governor_manual_recovery_ops::{
        LossGovernorManualRecoveryCommand, recover_loss_governor_manual_halt,
    },
};
use rust_decimal::Decimal;

const VALID_EVIDENCE_SHA256: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn enabled_kill_switch_config(state_path: &str) -> KillSwitchConfigBlock {
    KillSwitchConfigBlock {
        enabled: true,
        state_path: state_path.to_string(),
        max_state_file_bytes: 65_536,
        max_utc_daily_realized_loss: "250.00".to_string(),
        flatten_open_positions_on_breach: false,
        action_retry_interval_ms: 250,
        action_retry_timeout_ms: 5_000,
        mandatory_proof_max_age_ms: 1_000,
        manual_reset_evidence_max_age_ms: 60_000,
        forced_reduction_policy_sha256: VALID_EVIDENCE_SHA256.to_string(),
        forced_reduction_max_live_order_count: 4,
        forced_reduction_max_notional_per_order: "100.00".to_string(),
        authorized_operator_ids: vec!["operator-primary".to_string()],
        account_ids: vec!["POLYMARKET-001".to_string()],
        instrument_ids: vec!["BTC-USD.BINANCE".to_string()],
        cancel: None,
        flatten: None,
    }
}

fn loaded_with_enabled_loss_governor(
    state_path: &str,
) -> (
    bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    support::TempCaseDir,
) {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-loss-governor-manual-recovery");
    loaded.root_path = temp.path().join("root.toml");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    loaded.root.risk.kill_switch = Some(enabled_kill_switch_config(state_path));
    loaded.root.risk.live_submit_governance = Some(LiveSubmitGovernanceBlock {
        mode: LiveSubmitGovernanceMode::SupervisedDepositCapped,
    });
    (loaded, temp)
}

fn runtime_store(loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config) -> KillSwitchStore {
    let kill_switch = loaded
        .root
        .risk
        .kill_switch
        .as_ref()
        .expect("test enables kill-switch config");
    KillSwitchStore::from_root_config_path(&loaded.root_path, kill_switch)
}

fn zero_loss_snapshot() -> KillSwitchLossProtectionSnapshot {
    KillSwitchLossProtectionSnapshot {
        daily_bucket: Some(19_875),
        daily_realized_pnl: Decimal::ZERO,
        settlement_currency: Some("USDC".to_string()),
        cumulative_position_pnl: BTreeMap::new(),
        closed_position_pnl: BTreeMap::new(),
        adjusted_position_pnl: BTreeMap::new(),
        pending_halt_actions: None,
    }
}

fn loss_governor_halted_state() -> KillSwitchState {
    KillSwitchState::Halted {
        halt_id: "halt-loss-governor-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "loss-governor",
            2_000,
            "daily loss cap breached",
        ),
    }
}

fn valid_command() -> LossGovernorManualRecoveryCommand {
    LossGovernorManualRecoveryCommand {
        operator_id: "operator-primary".to_string(),
        evidence_path: "loss-governor/manual-recovery.json".to_string(),
        evidence_sha256: VALID_EVIDENCE_SHA256.to_string(),
        observed_at_ns: 2_500,
        now_ns: 2_600,
    }
}

#[test]
fn valid_manual_recovery_records_armed_state_and_audit_history() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    store
        .write_state_with_loss_snapshot(&loss_governor_halted_state(), Some(&zero_loss_snapshot()))
        .expect("latched loss-governor halt should persist");

    let outcome = recover_loss_governor_manual_halt(&loaded, valid_command())
        .expect("valid manual recovery should persist armed state");

    assert_eq!(outcome.previous_state, KillSwitchStateKind::Halted);
    assert_eq!(outcome.recovered_state, KillSwitchStateKind::Armed);
    assert_eq!(outcome.manual_recovery_count, 1);
    let record = store
        .load_recovery_record()
        .expect("recovered state should load");
    assert_eq!(
        record.recovery_state,
        KillSwitchRecoveryState::Recovered(KillSwitchState::Armed)
    );
    assert_eq!(record.loss_protection, Some(zero_loss_snapshot()));
    assert_eq!(record.loss_governor_manual_recoveries.len(), 1);
    assert_eq!(
        record.loss_governor_manual_recoveries[0].evidence_sha256,
        VALID_EVIDENCE_SHA256
    );

    let refreshed_snapshot = KillSwitchLossProtectionSnapshot {
        daily_realized_pnl: Decimal::new(1, 0),
        ..zero_loss_snapshot()
    };
    store
        .write_state_with_loss_snapshot(&KillSwitchState::Armed, Some(&refreshed_snapshot))
        .expect("later runtime loss snapshot write should persist");
    let refreshed_record = store
        .load_recovery_record()
        .expect("refreshed state should load");
    assert_eq!(refreshed_record.loss_protection, Some(refreshed_snapshot));
    assert_eq!(refreshed_record.loss_governor_manual_recoveries.len(), 1);
    assert_eq!(
        refreshed_record.loss_governor_manual_recoveries[0].evidence_sha256,
        VALID_EVIDENCE_SHA256
    );
}

#[test]
fn invalid_manual_recovery_evidence_leaves_loss_halt_latched() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halted = loss_governor_halted_state();
    store
        .write_state_with_loss_snapshot(&halted, Some(&zero_loss_snapshot()))
        .expect("latched loss-governor halt should persist");
    let mut command = valid_command();
    command.evidence_sha256 = "not-a-sha256".to_string();

    let error = recover_loss_governor_manual_halt(&loaded, command)
        .expect_err("invalid evidence must be refused");

    assert!(
        error
            .to_string()
            .contains("invalid manual recovery evidence"),
        "error must identify the refused evidence, got: {error}"
    );
    assert_eq!(
        store
            .load_recovery_state()
            .expect("store should remain readable"),
        KillSwitchRecoveryState::Recovered(halted)
    );
}

#[test]
fn manual_recovery_refuses_non_loss_governor_halt_without_downgrading() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let venue_truth_halt = KillSwitchState::Halted {
        halt_id: "halt-venue-truth-1".to_string(),
        trigger: KillSwitchHaltTrigger::venue_truth_divergence(
            "venue-truth",
            2_000,
            "causal venue truth diverged",
        ),
    };
    store
        .write_state_with_loss_snapshot(&venue_truth_halt, Some(&zero_loss_snapshot()))
        .expect("latched venue-truth halt should persist");

    let error = recover_loss_governor_manual_halt(&loaded, valid_command())
        .expect_err("loss-governor recovery must not clear another halt class");

    assert!(
        error
            .to_string()
            .contains("refusing to recover non-loss-governor kill-switch state"),
        "error must name the no-downgrade refusal, got: {error}"
    );
    assert_eq!(
        store
            .load_recovery_state()
            .expect("store should remain readable"),
        KillSwitchRecoveryState::Recovered(venue_truth_halt)
    );
}

#[test]
fn manual_recovery_refuses_missing_store_without_bootstrapping() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/missing-kill-switch.json");

    let error = recover_loss_governor_manual_halt(&loaded, valid_command())
        .expect_err("missing durable store must fail closed");

    assert!(
        error
            .to_string()
            .contains("kill-switch state file is missing"),
        "error must clearly identify missing durable state, got: {error}"
    );
    assert!(
        !runtime_store(&loaded).path().exists(),
        "manual recovery must not auto-create a missing kill-switch store"
    );
}
