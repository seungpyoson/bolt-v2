mod support;

use bolt_v2::{
    bolt_v3_config::{KillSwitchConfigBlock, load_bolt_v3_config},
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState, KillSwitchStateKind},
    bolt_v3_kill_switch_store::KillSwitchStore,
    bolt_v3_live_node::build_bolt_v3_live_node_with,
};
use nautilus_model::enums::TradingState;

fn enabled_kill_switch_config(state_path: &str) -> KillSwitchConfigBlock {
    KillSwitchConfigBlock {
        enabled: true,
        state_path: state_path.to_string(),
        max_state_file_bytes: 65_536,
        action_retry_interval_ms: 250,
        action_retry_timeout_ms: 5_000,
        mandatory_proof_max_age_ms: 1_000,
        manual_reset_evidence_max_age_ms: 60_000,
        forced_reduction_policy_sha256:
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        forced_reduction_max_live_order_count: 4,
        forced_reduction_max_notional_per_order: "100.00".to_string(),
        authorized_operator_ids: vec!["operator-primary".to_string()],
        account_ids: vec!["POLYMARKET-001".to_string()],
        instrument_ids: vec!["BTC-USD.BINANCE".to_string()],
        cancel: None,
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
    (loaded, temp)
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
fn recovered_kill_switch_state_seeds_submit_admission_latch_before_strategy_registration() {
    for (state, expected_kind) in recovered_runtime_latch_states() {
        let (loaded, _temp) = loaded_with_enabled_kill_switch("state/kill-switch.json");
        let kill_switch = loaded
            .root
            .risk
            .kill_switch
            .as_ref()
            .expect("test enables kill-switch config");
        let store = KillSwitchStore::from_root_config_path(&loaded.root_path, kill_switch);
        store
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
        let kill_switch = loaded
            .root
            .risk
            .kill_switch
            .as_ref()
            .expect("test enables kill-switch config");
        let store = KillSwitchStore::from_root_config_path(&loaded.root_path, kill_switch);
        store
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
        (
            KillSwitchState::Flat {
                halt_id: "halt-runtime-1".to_string(),
            },
            KillSwitchStateKind::Flat,
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
