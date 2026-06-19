use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

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

fn root_with_kill_switch(block: &str) -> String {
    format!("{}\n{block}", fixture_without_kill_switch())
}

fn valid_kill_switch_block_without_cancel() -> &'static str {
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

fn valid_kill_switch_block() -> String {
    format!(
        "{}\n{}",
        valid_kill_switch_block_without_cancel(),
        r#"
[risk.kill_switch.cancel]
enabled = true
retry_max_attempts = 3
retry_timeout_ms = 5000
retry_backoff_ms = 250
source_freshness_max_age_ms = 1000
mandatory_surfaces = [
  "open",
  "inflight",
  "pending-cancel",
  "emulated",
  "algorithm-managed",
  "contingent",
  "accepted-but-not-terminal",
]

[risk.kill_switch.flatten]
enabled = true
retry_max_attempts = 3
retry_timeout_ms = 5000
retry_backoff_ms = 250
source_freshness_max_age_ms = 1000
max_position_proof_age_ms = 1000
route_kind = "live_node_command_router"
max_live_order_count = 2
max_notional_per_order = "50.00"
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = true
is_quote_quantity = false
"#
    )
}

#[test]
fn kill_switch_config_is_optional_and_parses_when_present() {
    let root_without: BoltV3RootConfig = toml::from_str(&fixture_without_kill_switch()).unwrap();
    assert!(root_without.risk.kill_switch.is_none());

    let shipped_root: BoltV3RootConfig =
        toml::from_str(include_str!("fixtures/bolt_v3/root.toml")).unwrap();
    let shipped_kill_switch = shipped_root
        .risk
        .kill_switch
        .as_ref()
        .expect("fixture should carry the disabled operator kill-switch block");
    assert!(!shipped_kill_switch.enabled);
    assert!(validate_root_only(&shipped_root).is_empty());

    let root_with: BoltV3RootConfig =
        toml::from_str(&root_with_kill_switch(&valid_kill_switch_block())).unwrap();
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
    let cancel = kill_switch
        .cancel
        .as_ref()
        .expect("cancel block should parse");
    assert!(cancel.enabled);
    assert_eq!(cancel.retry_max_attempts, 3);
    assert_eq!(cancel.retry_timeout_ms, 5_000);
    assert_eq!(cancel.retry_backoff_ms, 250);
    assert_eq!(cancel.source_freshness_max_age_ms, 1_000);
    assert_eq!(cancel.mandatory_surfaces.len(), 7);
    let flatten = kill_switch
        .flatten
        .as_ref()
        .expect("flatten block should parse");
    assert!(flatten.enabled);
    assert_eq!(flatten.retry_max_attempts, 3);
    assert_eq!(flatten.retry_timeout_ms, 5_000);
    assert_eq!(flatten.retry_backoff_ms, 250);
    assert_eq!(flatten.source_freshness_max_age_ms, 1_000);
    assert_eq!(flatten.max_position_proof_age_ms, 1_000);
    assert_eq!(flatten.max_live_order_count, 2);
    assert_eq!(flatten.max_notional_per_order, "50.00");
    assert!(flatten.is_reduce_only);
    assert!(!flatten.is_quote_quantity);
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

#[test]
fn disabled_kill_switch_still_rejects_invalid_bootstrap_store_fields() {
    let block = valid_kill_switch_block_without_cancel()
        .replace("enabled = true", "enabled = false")
        .replace(
            "state_path = \"state/kill-switch.json\"",
            "state_path = \"\"",
        )
        .replace("max_state_file_bytes = 65536", "max_state_file_bytes = 0");
    let root: BoltV3RootConfig = toml::from_str(&root_with_kill_switch(&block)).unwrap();

    let errors = validate_root_only(&root);

    assert_eq!(
        errors,
        vec![
            "risk.kill_switch.state_path must be a non-empty relative path under the configured root"
                .to_string(),
            "risk.kill_switch.max_state_file_bytes must be positive".to_string(),
        ],
        "disabled kill-switch validation must only require bootstrap store fields"
    );
}

#[test]
fn enabled_kill_switch_cancel_requires_policy_fields_at_parse_time() {
    for (missing_field, cancel_block) in [
        (
            "retry_max_attempts",
            r#"
[risk.kill_switch.cancel]
enabled = true
retry_timeout_ms = 1000
retry_backoff_ms = 100
source_freshness_max_age_ms = 250
mandatory_surfaces = ["open"]
"#,
        ),
        (
            "retry_timeout_ms",
            r#"
[risk.kill_switch.cancel]
enabled = true
retry_max_attempts = 3
retry_backoff_ms = 100
source_freshness_max_age_ms = 250
mandatory_surfaces = ["open"]
"#,
        ),
        (
            "retry_backoff_ms",
            r#"
[risk.kill_switch.cancel]
enabled = true
retry_max_attempts = 3
retry_timeout_ms = 1000
source_freshness_max_age_ms = 250
mandatory_surfaces = ["open"]
"#,
        ),
        (
            "source_freshness_max_age_ms",
            r#"
[risk.kill_switch.cancel]
enabled = true
retry_max_attempts = 3
retry_timeout_ms = 1000
retry_backoff_ms = 100
mandatory_surfaces = ["open"]
"#,
        ),
        (
            "mandatory_surfaces",
            r#"
[risk.kill_switch.cancel]
enabled = true
retry_max_attempts = 3
retry_timeout_ms = 1000
retry_backoff_ms = 100
source_freshness_max_age_ms = 250
"#,
        ),
    ] {
        let block = format!(
            "{}\n{cancel_block}",
            valid_kill_switch_block_without_cancel()
        );
        let error = toml::from_str::<BoltV3RootConfig>(&root_with_kill_switch(&block))
            .expect_err("enabled cancel policy must require explicit configured fields")
            .to_string();

        assert!(
            error.contains("missing field"),
            "expected a missing field parse error for `{missing_field}`: {error}"
        );
        assert!(
            error.contains(missing_field),
            "expected `{missing_field}` in parse error: {error}"
        );
    }
}

#[test]
fn enabled_kill_switch_cancel_rejects_invalid_policy_values() {
    let block = format!(
        "{}\n{}",
        valid_kill_switch_block_without_cancel(),
        r#"
[risk.kill_switch.cancel]
enabled = true
retry_max_attempts = 0
retry_timeout_ms = 0
retry_backoff_ms = 0
source_freshness_max_age_ms = 0
mandatory_surfaces = ["open", "not-a-surface"]
"#
    );
    let root: BoltV3RootConfig = toml::from_str(&root_with_kill_switch(&block)).unwrap();
    let errors = validate_root_only(&root);

    for expected in [
        "risk.kill_switch.cancel.retry_max_attempts must be positive",
        "risk.kill_switch.cancel.retry_timeout_ms must be positive",
        "risk.kill_switch.cancel.retry_backoff_ms must be positive",
        "risk.kill_switch.cancel.source_freshness_max_age_ms must be positive",
        "risk.kill_switch.cancel.mandatory_surfaces must include every mandatory outstanding order risk surface",
        "risk.kill_switch.cancel.mandatory_surfaces[`not-a-surface`] is not a supported outstanding order risk surface",
    ] {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "expected `{expected}` in errors: {errors:?}"
        );
    }
}

#[test]
fn enabled_kill_switch_flatten_requires_policy_fields_at_parse_time() {
    for (missing_field, flatten_block) in [
        (
            "retry_max_attempts",
            r#"
[risk.kill_switch.flatten]
enabled = true
retry_timeout_ms = 1000
retry_backoff_ms = 100
source_freshness_max_age_ms = 250
max_position_proof_age_ms = 250
route_kind = "live_node_command_router"
max_live_order_count = 1
max_notional_per_order = "10.00"
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = true
is_quote_quantity = false
"#,
        ),
        (
            "max_position_proof_age_ms",
            r#"
[risk.kill_switch.flatten]
enabled = true
retry_max_attempts = 3
retry_timeout_ms = 1000
retry_backoff_ms = 100
source_freshness_max_age_ms = 250
route_kind = "live_node_command_router"
max_live_order_count = 1
max_notional_per_order = "10.00"
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = true
is_quote_quantity = false
"#,
        ),
        (
            "route_kind",
            r#"
[risk.kill_switch.flatten]
enabled = true
retry_max_attempts = 3
retry_timeout_ms = 1000
retry_backoff_ms = 100
source_freshness_max_age_ms = 250
max_position_proof_age_ms = 250
max_live_order_count = 1
max_notional_per_order = "10.00"
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = true
is_quote_quantity = false
"#,
        ),
    ] {
        let block = format!(
            "{}\n{}",
            valid_kill_switch_block_without_cancel(),
            flatten_block
        );
        let error = toml::from_str::<BoltV3RootConfig>(&root_with_kill_switch(&block))
            .expect_err("enabled flatten policy must require explicit configured fields")
            .to_string();

        assert!(
            error.contains("missing field"),
            "expected a missing field parse error for `{missing_field}`: {error}"
        );
        assert!(
            error.contains(missing_field),
            "expected `{missing_field}` in parse error: {error}"
        );
    }
}

#[test]
fn enabled_kill_switch_flatten_rejects_invalid_policy_values() {
    let block = format!(
        "{}\n{}",
        valid_kill_switch_block_without_cancel(),
        r#"
[risk.kill_switch.flatten]
enabled = true
retry_max_attempts = 0
retry_timeout_ms = 0
retry_backoff_ms = 0
source_freshness_max_age_ms = 0
max_position_proof_age_ms = 0
route_kind = "live_node_command_router"
max_live_order_count = 5
max_notional_per_order = "101.00"
order_type = "market"
time_in_force = "gtd"
is_post_only = true
is_reduce_only = false
is_quote_quantity = true
"#
    );
    let root: BoltV3RootConfig = toml::from_str(&root_with_kill_switch(&block)).unwrap();
    let errors = validate_root_only(&root);

    for expected in [
        "risk.kill_switch.flatten.retry_max_attempts must be positive",
        "risk.kill_switch.flatten.retry_timeout_ms must be positive",
        "risk.kill_switch.flatten.retry_backoff_ms must be positive",
        "risk.kill_switch.flatten.source_freshness_max_age_ms must be positive",
        "risk.kill_switch.flatten.max_position_proof_age_ms must be positive",
        "risk.kill_switch.flatten.max_live_order_count must be <= risk.kill_switch.forced_reduction_max_live_order_count",
        "risk.kill_switch.flatten.max_notional_per_order must be <= risk.kill_switch.forced_reduction_max_notional_per_order",
        "risk.kill_switch.flatten.is_reduce_only must be true",
        "risk.kill_switch.flatten.is_quote_quantity must be false",
        "risk.kill_switch.flatten: order_template.time_in_force=gtd is not supported for order_type=market",
        "risk.kill_switch.flatten: order_template.is_post_only must be false for order_type=market",
    ] {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "expected `{expected}` in errors: {errors:?}"
        );
    }
}
