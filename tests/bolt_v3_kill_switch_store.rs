use std::fs;

use bolt_v2::{
    bolt_v3_config::BoltV3RootConfig,
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState},
    bolt_v3_kill_switch_store::{
        KillSwitchRecoveryReason, KillSwitchRecoveryState, KillSwitchStore,
    },
};

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
    assert_eq!(persisted["schema_version"], 1);
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
