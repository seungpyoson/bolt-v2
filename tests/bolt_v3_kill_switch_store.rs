use std::{
    collections::BTreeMap,
    fs,
    sync::{Mutex, OnceLock},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use bolt_v2::{
    bolt_v3_config::BoltV3RootConfig,
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState},
    bolt_v3_kill_switch_store::{
        KILL_SWITCH_STORE_SCHEMA_VERSION, KillSwitchLossGovernorManualRecoveryRecord,
        KillSwitchLossProtectionSnapshot, KillSwitchRecoveryReason, KillSwitchRecoveryState,
        KillSwitchStore, KillSwitchStoreError,
    },
};
use rust_decimal::Decimal;

fn fixture_without_kill_switch() -> String {
    let mut fixture: toml::Value =
        toml::from_str(include_str!("fixtures/bolt_v3/root.toml")).unwrap();
    fixture
        .get_mut("risk")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture should have a risk table")
        .remove("kill_switch");
    toml::to_string(&fixture).expect("fixture without kill switch should serialize")
}

#[derive(Default)]
struct CapturingLogger {
    records: Mutex<Vec<(log::Level, String)>>,
}

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .push((record.level(), record.args().to_string()));
    }

    fn flush(&self) {}
}

impl CapturingLogger {
    fn reset(&self) {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .clear();
    }

    fn records(&self) -> Vec<(log::Level, String)> {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .clone()
    }
}

static CAPTURING_LOGGER: OnceLock<&'static CapturingLogger> = OnceLock::new();
static CAPTURING_LOGGER_OBSERVERS: Mutex<()> = Mutex::new(());

fn install_capturing_logger() -> &'static CapturingLogger {
    static INSTALL_OUTCOME: OnceLock<bool> = OnceLock::new();
    let logger = CAPTURING_LOGGER.get_or_init(|| Box::leak(Box::new(CapturingLogger::default())));
    let installed = *INSTALL_OUTCOME.get_or_init(|| log::set_logger(*logger).is_ok());
    assert!(
        installed,
        "capturing logger could not claim the global log slot; another logger is installed"
    );
    log::set_max_level(log::LevelFilter::Trace);
    logger
}

fn zero_loss_snapshot() -> KillSwitchLossProtectionSnapshot {
    KillSwitchLossProtectionSnapshot {
        daily_bucket: None,
        daily_realized_pnl: Decimal::ZERO,
        settlement_currency: None,
        cumulative_position_pnl: BTreeMap::new(),
        closed_position_pnl: BTreeMap::new(),
        adjusted_position_pnl: BTreeMap::new(),
        pending_halt_actions: None,
    }
}

#[test]
fn bootstrap_initial_armed_loss_snapshot_creates_recoverable_store() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("state/kill-switch.json");
    let store = KillSwitchStore::new(path, 65_536);

    store
        .bootstrap_initial_armed_loss_snapshot()
        .expect("initial armed store should bootstrap");

    let record = store
        .load_recovery_record()
        .expect("bootstrapped store should recover");
    assert_eq!(
        record.recovery_state,
        KillSwitchRecoveryState::Recovered(KillSwitchState::Armed)
    );
    assert_eq!(
        record.loss_protection,
        Some(KillSwitchLossProtectionSnapshot {
            daily_bucket: None,
            daily_realized_pnl: Decimal::ZERO,
            settlement_currency: None,
            cumulative_position_pnl: BTreeMap::new(),
            closed_position_pnl: BTreeMap::new(),
            adjusted_position_pnl: BTreeMap::new(),
            pending_halt_actions: None,
        })
    );
}

#[test]
fn bootstrap_initial_armed_loss_snapshot_refuses_to_overwrite_existing_store() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("state/kill-switch.json");
    let store = KillSwitchStore::new(path.clone(), 65_536);
    let existing = KillSwitchState::Flat {
        halt_id: "halt-1".to_string(),
    };
    store
        .write_state(&existing)
        .expect("existing state should persist");

    let error = store
        .bootstrap_initial_armed_loss_snapshot()
        .expect_err("bootstrap must not overwrite an existing store");
    assert!(matches!(
        error,
        KillSwitchStoreError::StateAlreadyExists { path: error_path } if error_path == path
    ));
    assert_eq!(
        store
            .load_recovery_state()
            .expect("existing state should remain recoverable"),
        KillSwitchRecoveryState::Recovered(existing)
    );
}

#[test]
fn bootstrap_initial_armed_loss_snapshot_rejects_oversized_initial_state_before_write() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("state/kill-switch.json");
    let store = KillSwitchStore::new(path.clone(), 1);

    let error = store
        .bootstrap_initial_armed_loss_snapshot()
        .expect_err("bootstrap must enforce the configured state size limit before writing");

    assert!(matches!(
        error,
        KillSwitchStoreError::StateTooLarge {
            path: error_path,
            bytes,
            max_bytes: 1
        } if error_path == path && bytes > 1
    ));
    assert!(
        !path.exists(),
        "oversized bootstrap state must not create partial evidence"
    );
}

#[test]
fn bootstrap_initial_armed_loss_snapshot_preserves_blocking_parent_file_on_path_error() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let parent = temp.path().join("state");
    fs::write(&parent, b"not-a-directory").expect("blocking parent file should write");
    let path = parent.join("kill-switch.json");
    let store = KillSwitchStore::new(path.clone(), 65_536);

    let error = store
        .bootstrap_initial_armed_loss_snapshot()
        .expect_err("bootstrap must surface path errors without replacing existing files");

    assert!(
        matches!(error, KillSwitchStoreError::Io { .. }),
        "expected path error, got: {error:?}"
    );
    assert_eq!(
        fs::read(&parent).expect("blocking parent file should remain"),
        b"not-a-directory"
    );
    assert!(
        !path.exists(),
        "path-error bootstrap must not create store evidence"
    );
}

#[test]
fn missing_corrupt_or_unresolved_evidence_recovers_fail_closed() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("kill-switch-state.json");
    let store = KillSwitchStore::new(path.clone(), 65_536);

    assert_eq!(
        store
            .load_recovery_state()
            .expect("missing load should classify"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::MissingEvidence,
            state: None,
        }
    );

    fs::write(&path, "not-json").expect("corrupt state should write");
    assert_eq!(
        store
            .load_recovery_state()
            .expect("corrupt load should classify"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::CorruptEvidence,
            state: None,
        }
    );

    let unresolved = KillSwitchState::Halting {
        halt_id: "halt-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "bolt_v3.loss_governor",
            1_717_200_000_000_000_000,
            "daily_realized_loss_limit",
        ),
    };
    store
        .write_state(&unresolved)
        .expect("unresolved state should persist");

    assert_eq!(
        store
            .load_recovery_state()
            .expect("unresolved load should classify"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::UnresolvedHalt,
            state: Some(unresolved),
        }
    );
}

#[test]
fn overlapping_loss_snapshot_position_maps_recover_corrupt_fail_closed() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("kill-switch-state.json");
    let store = KillSwitchStore::new(path.clone(), 65_536);

    let persisted = serde_json::json!({
        "schema_version": KILL_SWITCH_STORE_SCHEMA_VERSION,
        "state": "Armed",
        "loss_protection": {
            "daily_bucket": 19_875,
            "daily_realized_pnl": "-10",
            "settlement_currency": "USDC",
            "cumulative_position_pnl": {
                "P-001": {
                    "realized_pnl": "-10",
                    "last_observed_at_unix_nanos": 1_717_200_000_000_000_000_u64
                }
            },
            "closed_position_pnl": {
                "P-001": {
                    "realized_pnl": "-10",
                    "last_observed_at_unix_nanos": 1_717_200_000_000_000_000_u64
                }
            },
            "adjusted_position_pnl": {}
        }
    });
    fs::write(
        &path,
        serde_json::to_vec_pretty(&persisted).expect("test json should serialize"),
    )
    .expect("corrupt semantic state should write");

    assert_eq!(
        store
            .load_recovery_state()
            .expect("overlapping snapshot maps should classify"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::CorruptEvidence,
            state: Some(KillSwitchState::Armed),
        }
    );
}

#[test]
fn loss_snapshot_missing_settlement_currency_recovers_corrupt_fail_closed() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("kill-switch-state.json");
    let store = KillSwitchStore::new(path.clone(), 65_536);

    let persisted = serde_json::json!({
        "schema_version": KILL_SWITCH_STORE_SCHEMA_VERSION,
        "state": "Armed",
        "loss_protection": {
            "daily_bucket": 19_875,
            "daily_realized_pnl": "-10",
            "cumulative_position_pnl": {},
            "closed_position_pnl": {},
            "adjusted_position_pnl": {}
        }
    });
    fs::write(
        &path,
        serde_json::to_vec_pretty(&persisted).expect("test json should serialize"),
    )
    .expect("corrupt semantic state should write");

    assert_eq!(
        store
            .load_recovery_state()
            .expect("missing settlement currency should classify"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::CorruptEvidence,
            state: Some(KillSwitchState::Armed),
        }
    );
}

#[test]
fn loss_snapshot_unknown_settlement_currency_recovers_corrupt_fail_closed() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("kill-switch-state.json");
    let store = KillSwitchStore::new(path.clone(), 65_536);

    let persisted = serde_json::json!({
        "schema_version": KILL_SWITCH_STORE_SCHEMA_VERSION,
        "state": "Armed",
        "loss_protection": {
            "daily_bucket": 19_875,
            "daily_realized_pnl": "-10",
            "settlement_currency": "NOT_A_REGISTERED_CURRENCY",
            "cumulative_position_pnl": {},
            "closed_position_pnl": {},
            "adjusted_position_pnl": {}
        }
    });
    fs::write(
        &path,
        serde_json::to_vec_pretty(&persisted).expect("test json should serialize"),
    )
    .expect("corrupt semantic state should write");

    assert_eq!(
        store
            .load_recovery_state()
            .expect("unknown settlement currency should classify"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::CorruptEvidence,
            state: Some(KillSwitchState::Armed),
        }
    );
}

#[test]
fn persisted_state_round_trips_with_schema_version() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("kill-switch-state.json");
    let store = KillSwitchStore::new(path.clone(), 65_536);
    let state = KillSwitchState::Flat {
        halt_id: "halt-1".to_string(),
    };

    store.write_state(&state).expect("state should persist");

    assert_eq!(
        store.load_recovery_state().expect("state should load"),
        KillSwitchRecoveryState::Recovered(state)
    );

    let persisted: serde_json::Value =
        serde_json::from_slice(&fs::read(path).expect("state file should read"))
            .expect("state file should be json");
    assert_eq!(
        persisted["schema_version"],
        KILL_SWITCH_STORE_SCHEMA_VERSION
    );
}

#[cfg(unix)]
#[test]
fn write_state_logs_when_manual_recovery_history_cannot_be_preserved() {
    let _logger_guard = CAPTURING_LOGGER_OBSERVERS
        .lock()
        .expect("capturing logger observer mutex poisoned");
    let logger = install_capturing_logger();
    logger.reset();

    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("kill-switch-state.json");
    let store = KillSwitchStore::new(path.clone(), 65_536);
    let loss_snapshot = zero_loss_snapshot();
    store
        .bootstrap_initial_armed_loss_snapshot()
        .expect("initial armed store should bootstrap");
    let previous_record = store
        .load_recovery_record()
        .expect("bootstrapped record should load");
    store
        .write_loss_governor_manual_recovery(
            &previous_record,
            &KillSwitchState::Armed,
            &loss_snapshot,
            KillSwitchLossGovernorManualRecoveryRecord {
                operator_id: "operator-primary".to_string(),
                evidence_path: "loss-governor/manual-recovery.json".to_string(),
                evidence_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                observed_at_ns: 2_500,
                recorded_at_ns: 2_600,
            },
        )
        .expect("manual recovery audit should persist");
    assert_eq!(
        store
            .load_recovery_record()
            .expect("manual recovery record should load")
            .loss_governor_manual_recoveries
            .len(),
        1
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o200))
        .expect("store should be made unreadable while parent remains writable");

    store
        .write_state_with_loss_snapshot(&KillSwitchState::Armed, Some(&loss_snapshot))
        .expect("state write must still succeed when history preservation read fails");

    let recovered = store
        .load_recovery_record()
        .expect("rewritten store should recover");
    assert_eq!(
        recovered.recovery_state,
        KillSwitchRecoveryState::Recovered(KillSwitchState::Armed)
    );
    assert!(
        recovered.loss_governor_manual_recoveries.is_empty(),
        "read-error fallback writes the explicit empty audit list"
    );
    let records = logger.records();
    assert!(
        records.iter().any(|(level, message)| {
            *level == log::Level::Error
                && message.contains(&path.display().to_string())
                && message.contains("loss_governor_manual_recoveries=[]")
                && message
                    .contains("failed to preserve loss-governor manual recovery audit history")
        }),
        "expected loud manual recovery history drop log, got: {records:?}"
    );
}

#[test]
fn config_relative_state_path_recovers_missing_evidence_fail_closed() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let root_path = temp.path().join("root.toml");
    let root_toml = format!(
        "{}\n{}",
        fixture_without_kill_switch(),
        r#"
[risk.kill_switch]
enabled = true
state_path = "state/kill-switch.json"
max_state_file_bytes = 65536
max_utc_daily_realized_loss = "250.00"
flatten_open_positions_on_breach = false
action_retry_interval_ms = 250
action_retry_timeout_ms = 5000
mandatory_proof_max_age_ms = 1000
manual_reset_evidence_max_age_ms = 60000
forced_reduction_policy_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
forced_reduction_max_live_order_count = 4
forced_reduction_max_notional_per_order = "100.00"
authorized_operator_ids = ["operator-primary"]
account_ids = ["POLYMARKET-001"]
instrument_ids = ["BTC-USD.BINANCE"]
"#
    );
    fs::write(&root_path, &root_toml).expect("root config should write");
    let root: BoltV3RootConfig = toml::from_str(&root_toml).expect("root config should parse");
    let kill_switch = root
        .risk
        .kill_switch
        .as_ref()
        .expect("kill-switch block should parse");

    let store = KillSwitchStore::from_root_config_path(&root_path, kill_switch);

    assert_eq!(
        store.path(),
        temp.path().join("state/kill-switch.json").as_path()
    );
    assert_eq!(
        store
            .load_recovery_state()
            .expect("missing load should classify"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::MissingEvidence,
            state: None,
        }
    );
}

#[test]
fn failed_manual_intervention_evidence_recovers_fail_closed() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("kill-switch-state.json");
    let store = KillSwitchStore::new(path, 65_536);
    let state = KillSwitchState::FailedManualIntervention {
        halt_id: "halt-1".to_string(),
        reason: "fsync failed".to_string(),
    };

    store
        .write_state(&state)
        .expect("failed state should persist");

    assert_eq!(
        store
            .load_recovery_state()
            .expect("failed state should load"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::UnresolvedHalt,
            state: Some(state),
        }
    );
}

#[test]
fn oversized_evidence_recovers_fail_closed_without_unbounded_read() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("kill-switch-state.json");
    let store = KillSwitchStore::new(path.clone(), 8);

    fs::write(&path, br#"{"oversized":true}"#).expect("oversized state should write");

    assert_eq!(
        store
            .load_recovery_state()
            .expect("oversized load should classify"),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::OversizedEvidence,
            state: None,
        }
    );
}
