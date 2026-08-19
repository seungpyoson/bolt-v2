use crate::support;

use std::collections::BTreeMap;
use std::fs;

use bolt_v2::{
    bolt_v3_config::{
        KillSwitchConfigBlock, LiveSubmitGovernanceBlock, LiveSubmitGovernanceMode,
        load_bolt_v3_config,
    },
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState, KillSwitchStateKind},
    bolt_v3_kill_switch_store::{
        KillSwitchLossProtectionSnapshot, KillSwitchRecoveryReason, KillSwitchStore,
    },
    bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with},
};
use nautilus_model::enums::TradingState;
use rust_decimal::Decimal;

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
        authorized_operator_ids: vec!["operator-primary".to_string()],
        account_ids: vec!["POLYMARKET-001".to_string()],
        instrument_ids: vec!["BTC-USD.BINANCE".to_string()],
        cancel: None,
        flatten: None,
    }
}

fn loaded_with_enabled_kill_switch(
    state_path: &str,
) -> (
    bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    support::TempCaseDir,
) {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-kill-switch-runtime");
    loaded.root_path = temp.path().join("root.toml");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    loaded.root.risk.kill_switch = Some(enabled_kill_switch_config(state_path));
    // Isolate the kill switch's own NT trading-state contribution. The shared
    // fixture enables the loss governor, whose untrusted-snapshot startup
    // fail-safe (on_untrusted_snapshot_trading_state = "reducing") fires during
    // build (live_node handler(None, ..)) and would otherwise mask what the
    // kill switch alone syncs into the RiskEngine — e.g. forcing Reducing over
    // an Armed (healthy) recovery that must stay Active. The loss governor's own
    // startup behavior is covered by its #658 tests.
    loaded.root.risk.loss_governor = None;
    loaded.root.risk.live_submit_governance = Some(LiveSubmitGovernanceBlock {
        mode: LiveSubmitGovernanceMode::SupervisedDepositCapped,
    });
    support::current_evidence::prepare_current_evidence_generation(&loaded);
    (loaded, temp)
}

#[test]
fn temp_case_dir_name_includes_process_id() {
    let label = "bolt-v3-kill-switch-runtime";
    let temp = support::TempCaseDir::new(label);
    let file_name = temp
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .expect("temp case dir should have a UTF-8 file name");
    let expected_prefix = format!("bolt-v2-{label}-{}-", std::process::id());

    assert!(
        file_name.starts_with(&expected_prefix),
        "temp case dir name {file_name:?} should start with {expected_prefix:?}"
    );
}

#[test]
fn enabled_kill_switch_missing_durable_state_fails_closed_before_live_node_build() {
    let (loaded, _temp) = loaded_with_enabled_kill_switch("state/missing-kill-switch.json");

    let error = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect_err("enabled kill-switch must fail closed before live-node build");
    let rendered = error.to_string();

    assert!(
        rendered.contains("kill-switch"),
        "error should identify kill-switch recovery: {rendered}"
    );
    assert!(
        rendered.contains("missing evidence"),
        "error should identify missing durable state evidence: {rendered}"
    );
}

#[test]
fn disabled_kill_switch_does_not_require_durable_state() {
    let (mut loaded, _temp) = loaded_with_enabled_kill_switch("state/missing-kill-switch.json");
    loaded
        .root
        .risk
        .kill_switch
        .as_mut()
        .expect("test installs kill-switch config")
        .enabled = false;

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("disabled kill-switch must not read durable state");

    assert_eq!(runtime.kill_switch_state_kind(), KillSwitchStateKind::Armed);
}

#[test]
fn enabled_kill_switch_corrupt_durable_state_fails_closed_before_live_node_build() {
    let (loaded, _temp) = loaded_with_enabled_kill_switch("state/kill-switch.json");
    let store = runtime_store(&loaded);
    fs::create_dir_all(
        store
            .path()
            .parent()
            .expect("state file should have parent"),
    )
    .expect("state parent should create");
    fs::write(store.path(), "not-json").expect("corrupt state should write");

    let error = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect_err("corrupt durable state must fail closed before live-node build");

    assert_recovery_error(error, KillSwitchRecoveryReason::CorruptEvidence);
}

#[test]
fn enabled_kill_switch_unknown_or_unsupported_durable_state_fails_closed_before_live_node_build() {
    let cases = [
        (
            r#"{"schema_version":2,"state":{"Mystery":{"halt_id":"halt-runtime-1"}}}"#,
            KillSwitchRecoveryReason::CorruptEvidence,
        ),
        (
            r#"{"schema_version":3,"state":{"Flat":{"halt_id":"halt-runtime-1"}}}"#,
            KillSwitchRecoveryReason::UnsupportedSchemaVersion,
        ),
    ];

    for (contents, expected_reason) in cases {
        let (loaded, _temp) = loaded_with_enabled_kill_switch("state/kill-switch.json");
        let store = runtime_store(&loaded);
        fs::create_dir_all(
            store
                .path()
                .parent()
                .expect("state file should have parent"),
        )
        .expect("state parent should create");
        fs::write(store.path(), contents).expect("state fixture should write");

        let error =
            build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
                .expect_err("invalid durable state must fail closed before live-node build");

        assert_recovery_error(error, expected_reason);
    }
}

fn armed_zero_loss_snapshot() -> KillSwitchLossProtectionSnapshot {
    // Mirrors what the loss-protection runtime persists for a freshly armed
    // system that has not yet recorded any realized PnL. An armed kill switch
    // is only allowed to resume when it carries a loss snapshot, so recovery
    // tests must seed the snapshot the production writer always pairs with the
    // armed state.
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
fn recovered_armed_kill_switch_state_keeps_normal_nt_trading_state() {
    let (loaded, _temp) = loaded_with_enabled_kill_switch("state/kill-switch.json");
    runtime_store(&loaded)
        .write_state_with_loss_snapshot(&KillSwitchState::Armed, Some(&armed_zero_loss_snapshot()))
        .expect("armed state with loss snapshot should persist");

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("armed recovery should allow normal runtime build");

    assert_eq!(runtime.kill_switch_state_kind(), KillSwitchStateKind::Armed);
    assert_eq!(runtime.nt_trading_state(), TradingState::Active);
}

#[test]
fn recovered_armed_kill_switch_without_loss_snapshot_fails_closed_to_manual_intervention() {
    let (loaded, _temp) = loaded_with_enabled_kill_switch("state/kill-switch.json");
    runtime_store(&loaded)
        .write_state(&KillSwitchState::Armed)
        .expect("armed state without a loss snapshot should persist");

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("missing loss snapshot fails closed to a halted runtime, not a build error");

    assert_eq!(
        runtime.kill_switch_state_kind(),
        KillSwitchStateKind::FailedManualIntervention,
        "an armed durable state with no loss-protection snapshot must fail closed",
    );
    assert_eq!(
        runtime.nt_trading_state(),
        TradingState::Halted,
        "a fail-closed manual-intervention seed must also halt the NT trading state",
    );
}

#[test]
fn recovered_kill_switch_state_seeds_submit_admission_latch_before_strategy_registration() {
    for (state, expected_kind) in recovered_runtime_latch_states() {
        let (loaded, _temp) = loaded_with_enabled_kill_switch("state/kill-switch.json");
        runtime_store(&loaded)
            .write_state(&state)
            .expect("recovered state should persist");

        let runtime =
            build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
                .expect("recovered state should still allow runtime build for future kill actions");

        assert_eq!(
            runtime.kill_switch_state_kind(),
            expected_kind,
            "runtime admission latch should carry recovered state before strategy registration"
        );
    }
}

#[test]
fn recovered_kill_switch_state_syncs_nt_trading_state_without_reactivating() {
    for (state, expected_trading_state) in recovered_runtime_nt_trading_states() {
        let (loaded, _temp) = loaded_with_enabled_kill_switch("state/kill-switch.json");
        runtime_store(&loaded)
            .write_state(&state)
            .expect("recovered state should persist");

        let runtime =
            build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
                .expect("recovered state should still allow runtime build for future kill actions");

        assert_eq!(
            runtime.nt_trading_state(),
            expected_trading_state,
            "runtime should sync recovered kill-switch state into NT RiskEngine trading state"
        );
        assert_ne!(
            runtime.nt_trading_state(),
            TradingState::Active,
            "Phase 3 must not restore NT trading state to Active for recovered halt states"
        );
    }
}

fn recovered_runtime_latch_states() -> Vec<(KillSwitchState, KillSwitchStateKind)> {
    vec![
        (
            KillSwitchState::Halted {
                halt_id: "halt-runtime-1".to_string(),
                trigger: KillSwitchHaltTrigger::loss_governor_breach(
                    "loss-governor",
                    1_000,
                    "daily loss cap breached",
                ),
            },
            KillSwitchStateKind::Halted,
        ),
        (
            KillSwitchState::Flat {
                halt_id: "halt-runtime-1".to_string(),
            },
            KillSwitchStateKind::Flat,
        ),
        (
            KillSwitchState::Cancelling {
                halt_id: "halt-runtime-1".to_string(),
            },
            KillSwitchStateKind::Cancelling,
        ),
        (
            KillSwitchState::Flattening {
                halt_id: "halt-runtime-1".to_string(),
            },
            KillSwitchStateKind::Flattening,
        ),
    ]
}

fn recovered_runtime_nt_trading_states() -> Vec<(KillSwitchState, TradingState)> {
    vec![
        (halted_runtime_state(), TradingState::Reducing),
        (
            KillSwitchState::Cancelling {
                halt_id: "halt-runtime-1".to_string(),
            },
            TradingState::Reducing,
        ),
        (
            KillSwitchState::Flattening {
                halt_id: "halt-runtime-1".to_string(),
            },
            TradingState::Reducing,
        ),
        (
            KillSwitchState::Flat {
                halt_id: "halt-runtime-1".to_string(),
            },
            TradingState::Halted,
        ),
    ]
}

fn halted_runtime_state() -> KillSwitchState {
    KillSwitchState::Halted {
        halt_id: "halt-runtime-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "loss-governor",
            1_000,
            "daily loss cap breached",
        ),
    }
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

fn assert_recovery_error(error: BoltV3LiveNodeError, expected: KillSwitchRecoveryReason) {
    assert!(
        matches!(
            error,
            BoltV3LiveNodeError::KillSwitchRecovery { reason } if reason == expected
        ),
        "expected kill-switch recovery reason {expected:?}, got {error}"
    );
}
