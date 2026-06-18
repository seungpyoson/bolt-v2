use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

fn root_with_kill_switch(block: &str) -> String {
    format!("{}\n{block}", include_str!("fixtures/bolt_v3/root.toml"))
}

fn valid_kill_switch_block() -> &'static str {
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
}

#[test]
fn kill_switch_config_is_optional_and_parses_when_present() {
    let root_without: BoltV3RootConfig =
        toml::from_str(include_str!("fixtures/bolt_v3/root.toml")).unwrap();
    assert!(root_without.risk.kill_switch.is_none());

    let root_with: BoltV3RootConfig =
        toml::from_str(&root_with_kill_switch(valid_kill_switch_block())).unwrap();
    let kill_switch = root_with
        .risk
        .kill_switch
        .as_ref()
        .expect("kill-switch block should parse");

    assert!(kill_switch.enabled);
    assert_eq!(kill_switch.state_path, "state/kill-switch.json");
    assert_eq!(
        kill_switch.forced_reduction_policy_sha256,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(kill_switch.forced_reduction_max_live_order_count, 4);
    assert_eq!(
        kill_switch.forced_reduction_max_notional_per_order,
        "100.00"
    );
    assert!(validate_root_only(&root_with).is_empty());
}

#[test]
fn enabled_kill_switch_rejects_active_flatten_until_shared_execution_path_exists() {
    let block = valid_kill_switch_block().replace(
        "flatten_open_positions_on_breach = false",
        "flatten_open_positions_on_breach = true",
    );
    let root: BoltV3RootConfig = toml::from_str(&root_with_kill_switch(&block)).unwrap();

    let errors = validate_root_only(&root);

    assert!(
        errors.iter().any(|error| error
            .contains("risk.kill_switch.flatten_open_positions_on_breach=true is not supported")),
        "expected active-flatten validation error, got: {errors:?}"
    );
}

#[test]
fn enabled_kill_switch_rejects_invalid_runtime_settings() {
    let root: BoltV3RootConfig = toml::from_str(&root_with_kill_switch(
        r#"
[risk.kill_switch]
enabled = true
state_path = ""
max_state_file_bytes = 0
max_utc_daily_realized_loss = "0"
flatten_open_positions_on_breach = false
action_retry_interval_ms = 0
action_retry_timeout_ms = 0
mandatory_proof_max_age_ms = 0
manual_reset_evidence_max_age_ms = 0
forced_reduction_policy_sha256 = "not-a-sha"
forced_reduction_max_live_order_count = 0
forced_reduction_max_notional_per_order = "0"
authorized_operator_ids = []
account_ids = []
instrument_ids = ["not-an-instrument"]
"#,
    ))
    .unwrap();

    let errors = validate_root_only(&root);

    for expected in [
        "risk.kill_switch.state_path must be a non-empty relative path",
        "risk.kill_switch.max_state_file_bytes must be positive",
        "risk.kill_switch.max_utc_daily_realized_loss must be positive",
        "risk.kill_switch.action_retry_interval_ms must be positive",
        "risk.kill_switch.action_retry_timeout_ms must be positive",
        "risk.kill_switch.mandatory_proof_max_age_ms must be positive",
        "risk.kill_switch.manual_reset_evidence_max_age_ms must be positive",
        "risk.kill_switch.forced_reduction_policy_sha256 must be a 64-character SHA-256 hex digest",
        "risk.kill_switch.forced_reduction_max_live_order_count must be positive",
        "risk.kill_switch.forced_reduction_max_notional_per_order must be positive",
        "risk.kill_switch.authorized_operator_ids must not be empty when enabled",
        "risk.kill_switch.account_ids must not be empty when enabled",
        "risk.kill_switch.instrument_ids[`not-an-instrument`] is not a valid Nautilus instrument ID",
    ] {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "expected `{expected}` in errors: {errors:?}"
        );
    }
}
