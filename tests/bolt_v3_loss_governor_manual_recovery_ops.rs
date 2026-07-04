use crate::support;

use std::{collections::BTreeMap, fs, path::PathBuf};

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

fn loaded_with_daily_only_loss_governor(
    state_path: &str,
) -> (
    bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    support::TempCaseDir,
) {
    let (mut loaded, temp) = loaded_with_enabled_loss_governor(state_path);
    let loss_governor = loaded
        .root
        .risk
        .loss_governor
        .as_mut()
        .expect("test fixture enables loss governor");
    loss_governor.max_per_trade_loss = None;
    loss_governor.max_rolling_loss = None;
    loss_governor.max_drawdown = None;
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

fn manual_recovery_audit_path(store: &KillSwitchStore) -> PathBuf {
    let stem = store
        .path()
        .file_stem()
        .expect("test state path should have a file stem")
        .to_string_lossy();
    store
        .path()
        .with_file_name(format!("{stem}-manual-recoveries.jsonl"))
}

fn read_manual_recovery_audit_lines(store: &KillSwitchStore) -> Vec<serde_json::Value> {
    let audit_path = manual_recovery_audit_path(store);
    fs::read_to_string(&audit_path)
        .unwrap_or_else(|error| panic!("manual recovery audit file should read: {error}"))
        .lines()
        .map(|line| serde_json::from_str(line).expect("audit line should be JSON"))
        .collect()
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

fn breaching_loss_snapshot() -> KillSwitchLossProtectionSnapshot {
    KillSwitchLossProtectionSnapshot {
        daily_realized_pnl: Decimal::new(-750, 2),
        ..zero_loss_snapshot()
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
fn valid_manual_recovery_records_armed_state_and_append_only_audit_history() {
    let (loaded, _temp) = loaded_with_daily_only_loss_governor("state/kill-switch.json");
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
    let audit_lines = read_manual_recovery_audit_lines(&store);
    assert_eq!(audit_lines.len(), 1);
    assert_eq!(audit_lines[0]["evidence_sha256"], VALID_EVIDENCE_SHA256);
    let state_json: serde_json::Value =
        serde_json::from_slice(&fs::read(store.path()).expect("state file should read"))
            .expect("state file should be JSON");
    assert!(
        state_json.get("loss_governor_manual_recoveries").is_none(),
        "manual recovery audit history must not be stored in the kill-switch state file"
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
    assert_eq!(read_manual_recovery_audit_lines(&store).len(), 1);

    store
        .invalidate()
        .expect("state invalidation should not touch append-only audit history");
    assert_eq!(
        read_manual_recovery_audit_lines(&store)[0]["evidence_sha256"],
        VALID_EVIDENCE_SHA256
    );
}

#[test]
fn manual_recovery_refuses_missing_durable_dimensions_instead_of_fabricating_pass() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halted = loss_governor_halted_state();
    store
        .write_state_with_loss_snapshot(&halted, Some(&zero_loss_snapshot()))
        .expect("latched loss-governor halt should persist");

    let error = recover_loss_governor_manual_halt(&loaded, valid_command())
        .expect_err("missing durable dimensions must refuse manual recovery");

    let message = error.to_string();
    assert!(
        message.contains("missing-dimension fail-closed"),
        "refusal must identify the missing-dimension fail-closed check, got: {message}"
    );
    assert!(
        message.contains("per_trade_pnl"),
        "refusal must name the missing durable dimension, got: {message}"
    );
    assert_eq!(
        store
            .load_recovery_state()
            .expect("store should remain readable"),
        KillSwitchRecoveryState::Recovered(halted)
    );
}

#[test]
fn manual_recovery_refuses_still_breaching_stored_loss_with_diagnostic() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halted = loss_governor_halted_state();
    store
        .write_state_with_loss_snapshot(&halted, Some(&breaching_loss_snapshot()))
        .expect("latched loss-governor halt should persist");

    let error = recover_loss_governor_manual_halt(&loaded, valid_command())
        .expect_err("stored loss still breaching the limit must refuse manual recovery");

    let message = error.to_string();
    assert!(
        message.contains("daily_loss_limit"),
        "refusal must identify the daily loss check, got: {message}"
    );
    assert!(
        message.contains("-7.50") && message.contains("7.50"),
        "refusal must include stored loss and configured limit, got: {message}"
    );
}

#[test]
fn manual_recovery_refuses_unauthorized_operator_without_downgrading() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halted = loss_governor_halted_state();
    store
        .write_state_with_loss_snapshot(&halted, Some(&zero_loss_snapshot()))
        .expect("latched loss-governor halt should persist");
    let mut command = valid_command();
    command.operator_id = "operator-secondary".to_string();

    let error = recover_loss_governor_manual_halt(&loaded, command)
        .expect_err("unauthorized operator must be refused");

    assert!(
        error.to_string().contains("is not authorized"),
        "error must identify authorization refusal, got: {error}"
    );
    assert_eq!(
        store
            .load_recovery_state()
            .expect("store should remain readable"),
        KillSwitchRecoveryState::Recovered(halted)
    );
}

#[test]
fn manual_recovery_clears_loss_governor_halting_state_when_condition_has_passed() {
    let (loaded, _temp) = loaded_with_daily_only_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halting = KillSwitchState::Halting {
        halt_id: "halt-loss-governor-halting-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "loss-governor",
            2_000,
            "daily loss cap breached",
        ),
    };
    store
        .write_state_with_loss_snapshot(&halting, Some(&zero_loss_snapshot()))
        .expect("latched loss-governor halting state should persist");

    let outcome = recover_loss_governor_manual_halt(&loaded, valid_command())
        .expect("halting loss-governor state should recover when the condition has passed");

    assert_eq!(outcome.previous_state, KillSwitchStateKind::Halting);
    assert_eq!(
        store
            .load_recovery_state()
            .expect("store should recover after manual recovery"),
        KillSwitchRecoveryState::Recovered(KillSwitchState::Armed)
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
