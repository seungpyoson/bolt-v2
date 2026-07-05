use crate::support;

use std::{collections::BTreeMap, fs, path::PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use bolt_v2::{
    bolt_v3_config::{
        KillSwitchConfigBlock, LiveSubmitGovernanceBlock, LiveSubmitGovernanceMode,
        load_bolt_v3_config,
    },
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState, KillSwitchStateKind},
    bolt_v3_kill_switch_store::{
        KillSwitchLossProtectionSnapshot, KillSwitchRecoveryReason, KillSwitchRecoveryState,
        KillSwitchStore,
    },
    bolt_v3_loss_governor::LossHaltReason,
    bolt_v3_loss_governor_manual_recovery_ops::{
        LossGovernorManualRecoveryCommand, LossGovernorManualRecoveryError,
        recover_loss_governor_manual_halt,
    },
};
use rust_decimal::Decimal;

const VALID_EVIDENCE_SHA256: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NANOS_PER_UTC_DAY: u64 = 86_400_000_000_000;

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

fn assert_single_refused_audit(store: &KillSwitchStore, expected_reason_fragment: &str) {
    let audit_lines = read_manual_recovery_audit_lines(store);
    assert_eq!(audit_lines.len(), 1);
    assert_eq!(
        audit_lines[0]["outcome"],
        serde_json::Value::String("refused-with-reason".to_string())
    );
    assert!(
        audit_lines[0]["outcome_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains(expected_reason_fragment)),
        "refused audit must carry `{expected_reason_fragment}`, got: {}",
        audit_lines[0]
    );
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

fn persist_loss_governor_halted_state_with_reason(
    store: &KillSwitchStore,
    state: KillSwitchState,
    snapshot: &KillSwitchLossProtectionSnapshot,
    loss_halt_reason: LossHaltReason,
) {
    let state = match state {
        KillSwitchState::Halted { halt_id, trigger } => KillSwitchState::Halted {
            halt_id,
            trigger: KillSwitchHaltTrigger::loss_governor_breach_with_reason(
                trigger.source,
                trigger.source_timestamp_unix_nanos,
                trigger.reason,
                loss_halt_reason,
            ),
        },
        KillSwitchState::Halting { halt_id, trigger } => KillSwitchState::Halting {
            halt_id,
            trigger: KillSwitchHaltTrigger::loss_governor_breach_with_reason(
                trigger.source,
                trigger.source_timestamp_unix_nanos,
                trigger.reason,
                loss_halt_reason,
            ),
        },
        _ => panic!("test helper requires a latched halt state"),
    };
    store
        .write_state_with_loss_snapshot(&state, Some(snapshot))
        .expect("latched loss-governor halt should persist through typed serialization");
}

fn loss_governor_halted_state_at(observed_at_ns: u64) -> KillSwitchState {
    KillSwitchState::Halted {
        halt_id: "halt-loss-governor-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "loss-governor",
            observed_at_ns,
            "loss governor triggered",
        ),
    }
}

fn loss_governor_halted_state_with_reason_at(
    observed_at_ns: u64,
    loss_halt_reason: LossHaltReason,
) -> KillSwitchState {
    KillSwitchState::Halted {
        halt_id: "halt-loss-governor-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach_with_reason(
            "loss-governor",
            observed_at_ns,
            "loss governor triggered",
            loss_halt_reason,
        ),
    }
}

fn command_at(now_ns: u64) -> LossGovernorManualRecoveryCommand {
    LossGovernorManualRecoveryCommand {
        now_ns,
        observed_at_ns: now_ns,
        ..valid_command()
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
    let halt_observed_at_ns = 10 * NANOS_PER_UTC_DAY + 1_000;
    persist_loss_governor_halted_state_with_reason(
        &store,
        loss_governor_halted_state_at(halt_observed_at_ns),
        &zero_loss_snapshot(),
        LossHaltReason::DailyLossLimit,
    );

    let outcome =
        recover_loss_governor_manual_halt(&loaded, command_at(11 * NANOS_PER_UTC_DAY + 1_000))
            .expect("valid manual recovery should persist armed state");

    assert_eq!(outcome.previous_state, KillSwitchStateKind::Halted);
    assert_eq!(outcome.recovered_state, KillSwitchStateKind::Armed);
    assert_eq!(outcome.manual_recovery_count, 2);
    let record = store
        .load_recovery_record()
        .expect("recovered state should load");
    assert_eq!(
        record.recovery_state,
        KillSwitchRecoveryState::Recovered(KillSwitchState::Armed)
    );
    assert_eq!(record.loss_protection, Some(zero_loss_snapshot()));
    let audit_lines = read_manual_recovery_audit_lines(&store);
    assert_eq!(audit_lines.len(), 2);
    assert_eq!(audit_lines[0]["evidence_sha256"], VALID_EVIDENCE_SHA256);
    assert_eq!(
        audit_lines[0]["outcome"],
        serde_json::Value::String("attempted".to_string())
    );
    assert_eq!(
        audit_lines[1]["outcome"],
        serde_json::Value::String("recovered".to_string())
    );
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
    assert_eq!(read_manual_recovery_audit_lines(&store).len(), 2);

    store
        .invalidate()
        .expect("state invalidation should not touch append-only audit history");
    assert_eq!(
        read_manual_recovery_audit_lines(&store)[1]["evidence_sha256"],
        VALID_EVIDENCE_SHA256
    );
}

#[test]
fn manual_recovery_ignores_non_triggering_missing_dimensions_after_daily_clock_passes() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halt_observed_at_ns = 10 * NANOS_PER_UTC_DAY + 1_000;
    persist_loss_governor_halted_state_with_reason(
        &store,
        loss_governor_halted_state_at(halt_observed_at_ns),
        &zero_loss_snapshot(),
        LossHaltReason::DailyLossLimit,
    );

    let outcome =
        recover_loss_governor_manual_halt(&loaded, command_at(11 * NANOS_PER_UTC_DAY + 1_000))
            .expect(
                "non-triggering dimensions are re-checked by the runtime path at next node start",
            );

    assert_eq!(outcome.recovered_state, KillSwitchStateKind::Armed);
    assert_eq!(
        store
            .load_recovery_state()
            .expect("store should remain readable"),
        KillSwitchRecoveryState::Recovered(KillSwitchState::Armed)
    );
}

#[test]
fn manual_recovery_refuses_same_day_daily_halt_and_records_refused_audit() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halted = loss_governor_halted_state_at(10 * NANOS_PER_UTC_DAY + 1_000);
    persist_loss_governor_halted_state_with_reason(
        &store,
        halted,
        &breaching_loss_snapshot(),
        LossHaltReason::DailyLossLimit,
    );

    let error =
        recover_loss_governor_manual_halt(&loaded, command_at(10 * NANOS_PER_UTC_DAY + 2_000))
            .expect_err("same-day daily loss halt should remain latched");

    let message = error.to_string();
    assert!(
        message.contains("daily_loss_limit"),
        "refusal must identify the daily trigger clock check, got: {message}"
    );
    assert!(
        message.contains("UTC day has not rolled"),
        "refusal must identify the clock condition, got: {message}"
    );
    let audit_lines = read_manual_recovery_audit_lines(&store);
    assert_eq!(audit_lines.len(), 1);
    assert_eq!(
        audit_lines[0]["outcome"],
        serde_json::Value::String("refused-with-reason".to_string())
    );
    assert!(
        audit_lines[0]["outcome_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("daily_loss_limit")),
        "refused audit must carry the diagnostic, got: {}",
        audit_lines[0]
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
    let audit_lines = read_manual_recovery_audit_lines(&store);
    assert_eq!(audit_lines.len(), 1);
    assert_eq!(
        audit_lines[0]["outcome"],
        serde_json::Value::String("refused-with-reason".to_string())
    );
    assert!(
        audit_lines[0]["outcome_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("authorization refused")),
        "refused audit must carry the authorization diagnostic, got: {}",
        audit_lines[0]
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
    let halt_observed_at_ns = 10 * NANOS_PER_UTC_DAY + 1_000;
    let halting = KillSwitchState::Halting {
        halt_id: "halt-loss-governor-halting-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "loss-governor",
            halt_observed_at_ns,
            "daily loss cap breached",
        ),
    };
    persist_loss_governor_halted_state_with_reason(
        &store,
        halting,
        &zero_loss_snapshot(),
        LossHaltReason::DailyLossLimit,
    );

    let outcome =
        recover_loss_governor_manual_halt(&loaded, command_at(11 * NANOS_PER_UTC_DAY + 1_000))
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
fn manual_recovery_clears_daily_halt_after_utc_day_rolls() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halt_observed_at_ns = 10 * NANOS_PER_UTC_DAY + 1_000;
    persist_loss_governor_halted_state_with_reason(
        &store,
        loss_governor_halted_state_at(halt_observed_at_ns),
        &breaching_loss_snapshot(),
        LossHaltReason::DailyLossLimit,
    );

    let outcome =
        recover_loss_governor_manual_halt(&loaded, command_at(11 * NANOS_PER_UTC_DAY + 1_000))
            .expect("daily loss halt should clear after the UTC day rolls");

    assert_eq!(outcome.recovered_state, KillSwitchStateKind::Armed);
    assert_eq!(
        store
            .load_recovery_state()
            .expect("store should recover after manual recovery"),
        KillSwitchRecoveryState::Recovered(KillSwitchState::Armed)
    );
}

#[test]
fn manual_recovery_refuses_same_day_daily_halt() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halt_observed_at_ns = 10 * NANOS_PER_UTC_DAY + 1_000;
    let halted = loss_governor_halted_state_at(halt_observed_at_ns);
    persist_loss_governor_halted_state_with_reason(
        &store,
        halted.clone(),
        &breaching_loss_snapshot(),
        LossHaltReason::DailyLossLimit,
    );

    let error =
        recover_loss_governor_manual_halt(&loaded, command_at(10 * NANOS_PER_UTC_DAY + 2_000))
            .expect_err("same-day daily loss halt should remain latched");

    assert!(
        error.to_string().contains("daily_loss_limit"),
        "refusal should identify the daily trigger clock check, got: {error}"
    );
    assert_eq!(
        store
            .load_recovery_state()
            .expect("store should remain readable"),
        KillSwitchRecoveryState::Recovered(loss_governor_halted_state_with_reason_at(
            halt_observed_at_ns,
            LossHaltReason::DailyLossLimit,
        ))
    );
}

#[test]
fn manual_recovery_clears_rolling_halt_after_window_elapses() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halted = loss_governor_halted_state_at(2_000);
    persist_loss_governor_halted_state_with_reason(
        &store,
        halted,
        &breaching_loss_snapshot(),
        LossHaltReason::RollingLossLimit,
    );

    let outcome = recover_loss_governor_manual_halt(&loaded, command_at(300_000_002_001))
        .expect("rolling loss halt should clear after the configured window elapses");

    assert_eq!(outcome.recovered_state, KillSwitchStateKind::Armed);
}

#[test]
fn manual_recovery_refuses_drawdown_trigger_with_runtime_path_diagnostic() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halted = loss_governor_halted_state_at(2_000);
    persist_loss_governor_halted_state_with_reason(
        &store,
        halted,
        &breaching_loss_snapshot(),
        LossHaltReason::MaxDrawdownLimit,
    );

    let error = recover_loss_governor_manual_halt(&loaded, command_at(300_000_000_001))
        .expect_err("drawdown-triggered halt requires runtime-path recovery");

    let message = error.to_string();
    assert!(
        message.contains("max_drawdown_limit") && message.contains("runtime-path recovery"),
        "drawdown refusal should identify the runtime-path requirement, got: {message}"
    );
}

#[cfg(unix)]
#[test]
fn manual_recovery_state_write_failure_leaves_prewrite_audit_durable() {
    let (loaded, _temp) = loaded_with_daily_only_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halt_observed_at_ns = 10 * NANOS_PER_UTC_DAY + 1_000;
    persist_loss_governor_halted_state_with_reason(
        &store,
        loss_governor_halted_state_at(halt_observed_at_ns),
        &zero_loss_snapshot(),
        LossHaltReason::DailyLossLimit,
    );
    fs::write(manual_recovery_audit_path(&store), b"")
        .expect("empty audit file should pre-exist so append can succeed");
    let state_dir = store
        .path()
        .parent()
        .expect("state path should have parent")
        .to_path_buf();
    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o500))
        .expect("state dir should become read-only");

    let recovery_result =
        recover_loss_governor_manual_halt(&loaded, command_at(11 * NANOS_PER_UTC_DAY + 1_000));
    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))
        .expect("state dir permissions should restore for cleanup");
    let error = recovery_result.expect_err("state write should fail after the audit is durable");

    assert!(
        matches!(
            error,
            LossGovernorManualRecoveryError::StoreWriteFailed { .. }
                | LossGovernorManualRecoveryError::FailedStateWriteFailed { .. }
        ),
        "write failure should surface through the existing error path, got: {error}"
    );
    let audit_lines = read_manual_recovery_audit_lines(&store);
    assert!(
        audit_lines
            .iter()
            .any(|line| line["outcome"] == serde_json::Value::String("attempted".to_string())),
        "pre-write attempted audit outcome should be durable, got: {audit_lines:?}"
    );
    assert!(
        !audit_lines
            .iter()
            .any(|line| line["outcome"] == serde_json::Value::String("recovered".to_string())),
        "failed write must not leave a recovered audit outcome, got: {audit_lines:?}"
    );
    assert!(
        audit_lines
            .iter()
            .any(|line| line["outcome"] == serde_json::Value::String("write-failed".to_string())),
        "write-failed audit outcome should be durable, got: {audit_lines:?}"
    );
}

#[test]
fn manual_recovery_refuses_legacy_loss_governor_store_without_trigger_reason() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halted = loss_governor_halted_state_at(10 * NANOS_PER_UTC_DAY + 1_000);
    store
        .write_state_with_loss_snapshot(&halted, Some(&breaching_loss_snapshot()))
        .expect("legacy latched loss-governor halt should persist");

    let error =
        recover_loss_governor_manual_halt(&loaded, command_at(11 * NANOS_PER_UTC_DAY + 1_000))
            .expect_err("legacy store without typed trigger reason must refuse fail closed");

    assert!(
        error.to_string().contains("legacy") && error.to_string().contains("trigger reason"),
        "legacy refusal should identify missing typed trigger reason, got: {error}"
    );
    assert_single_refused_audit(&store, "legacy-store");
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
    assert_single_refused_audit(&store, "invalid manual recovery evidence");
}

#[test]
fn manual_recovery_refuses_missing_loss_snapshot_and_audits() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halted = loss_governor_halted_state_with_reason_at(2_000, LossHaltReason::DailyLossLimit);
    store
        .write_state(&halted)
        .expect("latched loss-governor halt without loss snapshot should persist");

    let error = recover_loss_governor_manual_halt(&loaded, valid_command())
        .expect_err("missing loss snapshot must be refused");

    assert!(
        error.to_string().contains("no loss-protection snapshot"),
        "error must identify missing loss snapshot, got: {error}"
    );
    assert_single_refused_audit(&store, "no loss-protection snapshot");
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
    assert_single_refused_audit(&store, "non-loss-governor");
}

#[test]
fn manual_recovery_refuses_unsupported_state_and_audits() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    store
        .write_state_with_loss_snapshot(&KillSwitchState::Armed, Some(&zero_loss_snapshot()))
        .expect("armed state should persist");

    let error = recover_loss_governor_manual_halt(&loaded, valid_command())
        .expect_err("unsupported manual recovery state must be refused");

    assert!(
        error
            .to_string()
            .contains("cannot recover kill-switch state Armed"),
        "error must name unsupported state, got: {error}"
    );
    assert_single_refused_audit(&store, "cannot recover kill-switch state Armed");
    assert_eq!(
        store
            .load_recovery_state()
            .expect("store should remain readable"),
        KillSwitchRecoveryState::Recovered(KillSwitchState::Armed)
    );
}

#[test]
fn manual_recovery_refuses_store_fail_closed_state_and_audits() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    store
        .write_state_with_loss_snapshot(
            &loss_governor_halted_state_with_reason_at(2_000, LossHaltReason::DailyLossLimit),
            Some(&zero_loss_snapshot()),
        )
        .expect("initial state should persist before invalidation");
    store
        .invalidate()
        .expect("test should corrupt the durable store");

    let error = recover_loss_governor_manual_halt(&loaded, valid_command())
        .expect_err("fail-closed store should refuse recovery");

    assert!(
        error.to_string().contains("fail-closed"),
        "error must identify store fail-closed refusal, got: {error}"
    );
    assert_single_refused_audit(&store, "fail-closed");
    assert_eq!(
        store
            .load_recovery_state()
            .expect("store should remain readable as fail-closed"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::CorruptEvidence,
            state: None,
        }
    );
}

#[test]
fn manual_recovery_refuses_store_load_io_error_and_audits() {
    let (loaded, _temp) = loaded_with_enabled_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    store
        .write_state_with_loss_snapshot(
            &loss_governor_halted_state_with_reason_at(2_000, LossHaltReason::DailyLossLimit),
            Some(&zero_loss_snapshot()),
        )
        .expect("initial state should persist before IO fault");
    fs::remove_file(store.path()).expect("state file should be removable");
    fs::create_dir(store.path()).expect("directory at state path should force a read IO error");

    let error = recover_loss_governor_manual_halt(&loaded, valid_command())
        .expect_err("hard store-load IO error should refuse recovery");

    assert!(
        matches!(error, LossGovernorManualRecoveryError::StoreLoad(_)),
        "hard store-load IO should remain the command error, got: {error}"
    );
    assert_single_refused_audit(&store, "kill-switch store load failed");
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
    assert_single_refused_audit(&runtime_store(&loaded), "kill-switch state file is missing");
}

#[test]
fn manual_recovery_torn_audit_requires_repair_without_touching_halt() {
    let (loaded, _temp) = loaded_with_daily_only_loss_governor("state/kill-switch.json");
    let store = runtime_store(&loaded);
    let halt_observed_at_ns = 10 * NANOS_PER_UTC_DAY + 1_000;
    let halted = loss_governor_halted_state_with_reason_at(
        halt_observed_at_ns,
        LossHaltReason::DailyLossLimit,
    );
    store
        .write_state_with_loss_snapshot(&halted, Some(&zero_loss_snapshot()))
        .expect("latched loss-governor halt should persist");
    fs::write(manual_recovery_audit_path(&store), b"{\"torn\":true}")
        .expect("test should create a torn final audit line");

    let error =
        recover_loss_governor_manual_halt(&loaded, command_at(11 * NANOS_PER_UTC_DAY + 1_000))
            .expect_err("torn audit file should require repair before retry");

    assert!(
        error
            .to_string()
            .contains("repair-the-audit-file-and-retry"),
        "torn audit error must direct repair and retry, got: {error}"
    );
    assert_eq!(
        store
            .load_recovery_state()
            .expect("halt state should remain readable"),
        KillSwitchRecoveryState::Recovered(halted)
    );
    assert_eq!(
        fs::read_to_string(manual_recovery_audit_path(&store))
            .expect("audit file should remain readable"),
        "{\"torn\":true}"
    );
}
