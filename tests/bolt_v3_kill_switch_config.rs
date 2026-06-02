use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

fn root_with_kill_switch(block: &str) -> String {
    format!("{}\n{block}", include_str!("fixtures/bolt_v3/root.toml"))
}

fn valid_kill_switch_block_without_cancel() -> &'static str {
    r#"
[risk.kill_switch]
enabled = true
state_path = "state/kill-switch.json"
max_state_file_bytes = 65536
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
"#
    )
}

#[test]
fn kill_switch_config_is_optional_and_parses_when_present() {
    let root_without: BoltV3RootConfig =
        toml::from_str(include_str!("fixtures/bolt_v3/root.toml")).unwrap();
    assert!(root_without.risk.kill_switch.is_none());

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
    assert!(validate_root_only(&root_with).is_empty());
}

#[test]
fn enabled_kill_switch_rejects_invalid_runtime_settings() {
    let root: BoltV3RootConfig = toml::from_str(&root_with_kill_switch(
        r#"
[risk.kill_switch]
enabled = true
state_path = ""
max_state_file_bytes = 0
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
